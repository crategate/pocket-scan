use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};

use crate::action::Action;

static GIF_BYTES: &[u8] = include_bytes!("../assets/pocket-sand.gif");

const TWEET_URL: &str = "https://api.x.com/2/tweets";
const UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";
const MAX_POSTS_PER_DAY: u32 = 300; // $0.015 cheap-mode posts; ~$4.50/day ceiling
const MIN_INTERVAL: Duration = Duration::from_secs(3);
const GIF_ENABLED: bool = true; // flip off if billing shows media posts counting as URL posts

struct Creds {
    consumer_key: String,
    consumer_secret: String,
    token: String,
    token_secret: String,
}

fn creds() -> Option<Creds> {
    Some(Creds {
        consumer_key: std::env::var("X_CONSUMER_KEY").ok()?,
        consumer_secret: std::env::var("X_CONSUMER_SECRET").ok()?,
        token: std::env::var("X_ACCESS_TOKEN").ok()?,
        token_secret: std::env::var("X_ACCESS_TOKEN_SECRET").ok()?,
    })
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// OAuth 1.0a signature; body params are excluded for JSON and multipart bodies per spec
fn oauth_header(c: &Creds, method: &str, url: &str) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ts = now.as_secs().to_string();
    let nonce = format!("{:x}{:x}", now.subsec_nanos(), std::process::id());

    let mut params = vec![
        ("oauth_consumer_key", pct(&c.consumer_key)),
        ("oauth_nonce", pct(&nonce)),
        ("oauth_signature_method", "HMAC-SHA1".into()),
        ("oauth_timestamp", ts.clone()),
        ("oauth_token", pct(&c.token)),
        ("oauth_version", "1.0".into()),
    ];
    params.sort();
    let param_string = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let base = format!("{}&{}&{}", method, pct(url), pct(&param_string));
    let key = format!("{}&{}", pct(&c.consumer_secret), pct(&c.token_secret));

    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    format!(
        "OAuth oauth_consumer_key=\"{}\", oauth_nonce=\"{}\", oauth_signature=\"{}\", oauth_signature_method=\"HMAC-SHA1\", oauth_timestamp=\"{}\", oauth_token=\"{}\", oauth_version=\"1.0\"",
        pct(&c.consumer_key),
        pct(&nonce),
        pct(&sig),
        ts,
        pct(&c.token)
    )
}

/// "https://qrstud.io/qrmnky" -> "qrstud dot io/qrmnky"  ($0.20 -> $0.015)
fn cheapen(payload: &str) -> String {
    payload
        .replace("https://", "")
        .replace("http://", "")
        .replace('.', " dot ")
}

/// Upload the gif once; returns media_id_string. Simple upload, fine for <15MB gifs.
async fn upload_gif(client: &reqwest::Client, creds: &Creds) -> color_eyre::Result<String> {
    let part = reqwest::multipart::Part::bytes(GIF_BYTES.to_vec())
        .file_name("pocket-sand.gif")
        .mime_str("image/gif")?;
    let form = reqwest::multipart::Form::new().part("media", part);
    let resp = client
        .post(UPLOAD_URL)
        .header(
            reqwest::header::AUTHORIZATION,
            oauth_header(creds, "POST", UPLOAD_URL),
        )
        .multipart(form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(color_eyre::eyre::eyre!(
            "gif upload failed {status}: {body}"
        ));
    }
    let json: serde_json::Value = resp.json().await?;
    let id = json["media_id_string"]
        .as_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("no media_id_string in upload response"))?
        .to_string();
    info!("gif uploaded, media_id {id}");
    Ok(id)
}

pub fn spawn(action_tx: UnboundedSender<Action>, mut outbound: UnboundedReceiver<String>) {
    tokio::spawn(async move {
        loop {
            let Some(c) = creds() else {
                warn!("X credentials not set — twitter worker idle (check .envrc.local)");
                let _ = action_tx.send(Action::XStatus(false));
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            };
            run(&action_tx, &mut outbound, c).await;
            return; // run() only returns when the channel closes (shutdown)
        }
    });
}

async fn run(
    action_tx: &UnboundedSender<Action>,
    outbound: &mut UnboundedReceiver<String>,
    creds: Creds,
) {
    let client = reqwest::Client::new();
    let mut posts_today = 0u32;
    let mut day = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86400;

    // Upload the gif once per session (media expires ~24h if unattached; we attach immediately)
    let mut media_id: Option<String> = None;
    if GIF_ENABLED {
        match upload_gif(&client, &creds).await {
            Ok(id) => {
                media_id = Some(id);
                let _ = action_tx.send(Action::XTx {
                    ok: true,
                    text: "gif uploaded".into(),
                });
            }
            Err(e) => error!("{e} — posting without gif"),
        }
    }
    let _ = action_tx.send(Action::XStatus(true));

    while let Some(payload) = outbound.recv().await {
        let today = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 86400;
        if today != day {
            day = today;
            posts_today = 0;
            if GIF_ENABLED {
                if let Ok(id) = upload_gif(&client, &creds).await {
                    media_id = Some(id);
                }
            }
        }
        if posts_today >= MAX_POSTS_PER_DAY {
            let _ = action_tx.send(Action::XTx {
                ok: false,
                text: "daily cap — dropped".into(),
            });
            continue;
        }

        let text = format!(
            "Pocket Scan!\nI just scanned this QR code at DefCon, it also sent over meshtastic:\n\n{}",
            cheapen(&payload)
        );
        let mut body = serde_json::json!({ "text": text });
        if let Some(id) = &media_id {
            body["media"] = serde_json::json!({ "media_ids": [id] });
        }

        match client
            .post(TWEET_URL)
            .header(
                reqwest::header::AUTHORIZATION,
                oauth_header(&creds, "POST", TWEET_URL),
            )
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                posts_today += 1;
                info!("posted to X ({posts_today} today)");
                let _ = action_tx.send(Action::XTx { ok: true, text });
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                error!("X post failed {status}: {body}");
                // If the media_id went stale, drop it and re-upload next cycle
                if body.contains("media") && body.contains("invalid") {
                    media_id = None;
                }
                let _ = action_tx.send(Action::XTx {
                    ok: false,
                    text: format!("HTTP {status}"),
                });
                if status == 429 {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
            Err(e) => {
                error!("X request error: {e}");
                let _ = action_tx.send(Action::XTx {
                    ok: false,
                    text: "request error".into(),
                });
            }
        }
        tokio::time::sleep(MIN_INTERVAL).await;
    }
}
