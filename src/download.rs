use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

use aes::cipher::{KeyIvInit, StreamCipher};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use id3::TagLike;
use sha2::{Digest, Sha256};

use crate::api::SunoClient;
use crate::api::types::{AlignedWord, Clip};
use crate::errors::CliError;

enum MediaDecryptor {
    Aes128(Box<ctr::Ctr128BE<aes::Aes128>>),
    Aes256(Box<ctr::Ctr128BE<aes::Aes256>>),
}

impl MediaDecryptor {
    fn new(key: &[u8], iv: &[u8]) -> Result<Self, CliError> {
        match key.len() {
            16 => ctr::Ctr128BE::<aes::Aes128>::new_from_slices(key, iv)
                .map(Box::new)
                .map(Self::Aes128)
                .map_err(|_| CliError::Download("invalid Suno media key or IV".into())),
            32 => ctr::Ctr128BE::<aes::Aes256>::new_from_slices(key, iv)
                .map(Box::new)
                .map(Self::Aes256)
                .map_err(|_| CliError::Download("invalid Suno media key or IV".into())),
            _ => Err(CliError::Download("unsupported Suno media key size".into())),
        }
    }

    fn apply(&mut self, data: &mut [u8]) -> Result<(), CliError> {
        let result = match self {
            Self::Aes128(cipher) => cipher.try_apply_keystream(data),
            Self::Aes256(cipher) => cipher.try_apply_keystream(data),
        };
        result.map_err(|_| CliError::Download("Suno media counter overflow".into()))
    }
}

fn unwrap_media_secret(wrapped: &str, clip_id: &str, jwt: &str) -> Result<Vec<u8>, CliError> {
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(wrapped)
        .map_err(|_| CliError::Download("invalid Suno media license encoding".into()))?;
    if wrapped.len() <= 12 {
        return Err(CliError::Download("invalid Suno media license".into()));
    }
    let user_key = Sha256::digest(jwt.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&user_key)
        .map_err(|_| CliError::Download("invalid Suno user key".into()))?;
    cipher
        .decrypt(
            Nonce::from_slice(&wrapped[..12]),
            Payload {
                msg: &wrapped[12..],
                aad: clip_id.as_bytes(),
            },
        )
        .map_err(|_| CliError::Download("could not decrypt Suno media license".into()))
}

fn audio_extension(content_type: Option<&str>, url: &str) -> &'static str {
    match content_type {
        Some("m4a-opus") | Some("m4a") => "m4a",
        Some("webm-opus") | Some("webm") => "webm",
        Some("wav") => "wav",
        Some("mp3") => "mp3",
        _ if url
            .split('?')
            .next()
            .is_some_and(|path| path.ends_with(".m4a")) =>
        {
            "m4a"
        }
        _ => "mp3",
    }
}

pub async fn download_clip(
    suno: &SunoClient,
    clip: &Clip,
    output_dir: &str,
    video: bool,
) -> Result<String, CliError> {
    let (url, ext, encrypted) = if video {
        (
            clip.video_url
                .as_deref()
                .ok_or_else(|| CliError::Download("no video URL available".into()))?,
            "mp4",
            false,
        )
    } else if let Some(media) = clip.audio_download_media() {
        (
            media.url.as_str(),
            audio_extension(media.content_type.as_deref(), &media.url),
            media.is_encrypted(),
        )
    } else {
        (
            clip.audio_download_url()
                .ok_or_else(|| CliError::Download("no audio URL available".into()))?,
            "mp3",
            false,
        )
    };

    let filename = clip_filename(&clip.title, &clip.id, ext);
    // Create the target dir up front: generation has already spent credits by
    // the time we download, so a missing `--download` dir must not error out.
    tokio::fs::create_dir_all(output_dir).await?;
    let path = Path::new(output_dir).join(&filename);
    // Stream into a sibling `.part` and only rename into place on full success,
    // so an interrupted or truncated transfer never leaves a file that looks
    // like a finished download.
    let part_path = path.with_extension(format!("{ext}.part"));

    // Suno's current player obtains per-clip AES material from this endpoint;
    // media entries carrying `encoding` are AES-CTR ciphertext.
    let mut decryptor = if encrypted {
        let (rights, jwt) = suno.media_rights(&clip.id).await?;
        let key = unwrap_media_secret(&rights.key, &clip.id, &jwt)?;
        let iv = unwrap_media_secret(&rights.iv, &clip.id, &jwt)?;
        Some(MediaDecryptor::new(&key, &iv)?)
    } else {
        None
    };

    // Bounded client: connect timeout, per-read inactivity timeout (catches a
    // stalled CDN mid-stream), and an overall cap. Without these a hung
    // connection would block the download forever.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(CliError::Http)?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(CliError::Http)?
        .error_for_status()
        .map_err(CliError::Http)?;

    let total = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb.set_message(filename.clone());

    let written = match stream_to_file(&part_path, resp, &pb, decryptor.as_mut()).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(e);
        }
    };

    // Reject a short read against the advertised size instead of tagging a
    // truncated MP3 downstream.
    if total > 0 && written != total {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(CliError::Download(format!(
            "incomplete download: received {written} of {total} bytes for {filename}"
        )));
    }

    if let Err(e) = tokio::fs::rename(&part_path, &path).await {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(e.into());
    }
    pb.finish_with_message("done");

    Ok(path.display().to_string())
}

/// Stream a response body to `part_path`, returning the byte count written.
/// The caller removes the partial file on any error.
async fn stream_to_file(
    part_path: &Path,
    resp: reqwest::Response,
    pb: &ProgressBar,
    mut decryptor: Option<&mut MediaDecryptor>,
) -> Result<u64, CliError> {
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::File::create(part_path).await?;
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let mut chunk = chunk.map_err(CliError::Http)?.to_vec();
        pb.inc(chunk.len() as u64);
        written += chunk.len() as u64;
        if let Some(decryptor) = decryptor.as_deref_mut() {
            decryptor.apply(&mut chunk)?;
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(written)
}

/// Build `<title-slug>-<id8>.<ext>`. Runs of non-alphanumeric chars collapse
/// to a single `-` (a naive `replace("--", "-")` leaves `--` behind for 3+
/// char runs), and empty/symbol-only titles must not yield a leading dash.
fn clip_filename(title: &str, id: &str, ext: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    // Clip IDs are ASCII UUIDs, so a byte slice is safe here.
    let short_id = &id[..8.min(id.len())];
    if slug.is_empty() {
        format!("{short_id}.{ext}")
    } else {
        format!("{slug}-{short_id}.{ext}")
    }
}

/// Embed lyrics and metadata into an MP3 file using ID3v2 tags.
/// - USLT: unsynchronized (plain) lyrics — shown in most players
/// - SYLT: synchronized lyrics with word timestamps — shown in Apple Music, Spotify, etc.
/// - TIT2: title, TPE1: artist
pub fn embed_lyrics_in_mp3(
    mp3_path: &str,
    title: &str,
    plain_lyrics: Option<&str>,
    aligned_words: Option<&[AlignedWord]>,
) -> Result<(), CliError> {
    let mut tag = id3::Tag::read_from_path(mp3_path).unwrap_or_else(|_| id3::Tag::new());

    // Set title
    tag.set_title(title);

    // Plain lyrics (USLT) — shown in most players
    if let Some(lyrics) = plain_lyrics {
        tag.add_frame(id3::frame::Lyrics {
            lang: "eng".to_string(),
            description: String::new(),
            text: lyrics.to_string(),
        });
    }

    // Synchronized lyrics (SYLT) — timed word-by-word display
    if let Some(words) = aligned_words {
        let content: Vec<(u32, String)> = words
            .iter()
            .filter(|w| w.success)
            .map(|w| ((w.start_s * 1000.0) as u32, w.word.clone()))
            .collect();

        if !content.is_empty() {
            tag.add_frame(id3::frame::SynchronisedLyrics {
                lang: "eng".to_string(),
                timestamp_format: id3::frame::TimestampFormat::Ms,
                content_type: id3::frame::SynchronisedLyricsType::Lyrics,
                description: String::new(),
                content,
            });
        }
    }

    tag.write_to_path(mp3_path, id3::Version::Id3v24)
        .map_err(|e| CliError::Download(format!("failed to write ID3 tags: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_collapses_separator_runs() {
        assert_eq!(
            clip_filename("Hello, World!", "0123456789abcdef", "mp3"),
            "hello-world-01234567.mp3"
        );
        // 3+ char symbol run — the old replace("--","-") left "--" behind.
        assert_eq!(
            clip_filename("a — b", "0123456789abcdef", "mp3"),
            "a-b-01234567.mp3"
        );
    }

    #[test]
    fn filename_handles_empty_and_symbol_only_titles() {
        // No leading dash when the title slugs away to nothing.
        assert_eq!(clip_filename("", "0123456789abcdef", "mp3"), "01234567.mp3");
        assert_eq!(
            clip_filename("!!!", "0123456789abcdef", "mp4"),
            "01234567.mp4"
        );
    }

    #[test]
    fn filename_keeps_unicode_titles() {
        assert_eq!(
            clip_filename("夜の歌 Remix", "0123456789abcdef", "mp3"),
            "夜の歌-remix-01234567.mp3"
        );
        assert_eq!(clip_filename("short", "abc", "mp3"), "short-abc.mp3");
    }

    #[test]
    fn audio_extension_uses_the_media_container() {
        assert_eq!(audio_extension(Some("m4a-opus"), "https://cdn/clip"), "m4a");
        assert_eq!(audio_extension(Some("mp3"), "https://cdn/clip"), "mp3");
        assert_eq!(
            audio_extension(None, "https://cdn/clip.m4a?token=redacted"),
            "m4a"
        );
    }
}
