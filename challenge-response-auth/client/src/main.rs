#![forbid(unsafe_code)]
//! Async demo client (`reqwest`): happy path / tampering / replay.

use chal_auth_shared::{
    hmac_sha256_sign, AuthAttemptJson, AuthFailureJson, AuthSuccessJson, ChallengeJson,
};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "chal-auth-client", about = "HMAC challenge–response demo CLI")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:9393")]
    base_url: String,
    /// Same secret as server's `CHAL_AUTH_SECRET_HEX` (hex-encoded bytes).
    #[arg(long)]
    secret_hex: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Полный успешный обмен (`/v1/challenge` → подпись → `/v1/authenticate`).
    AuthenticateOk,
    /// Подделка MAC на последнем шаге (`signature mismatch`).
    AuthenticateTamperedMac,
    /// Отправить **дважды** один и тот же корректный proof (второй — replay).
    ReplaySameProofTwice,
}

fn secret_from_hex(secret_hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = secret_hex.trim().replace([' ', ':', '-'], "");
    hex::decode(&trimmed).map_err(|e| format!("invalid secret hex: {e}"))
}

fn base(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_string()
}

async fn fetch_challenge(cli: &reqwest::Client, base_url: &str) -> Result<ChallengeJson, String> {
    let url = format!("{}/v1/challenge", base(base_url));
    let res = cli
        .post(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!(
            "challenge HTTP {} {}",
            res.status(),
            res.text().await.unwrap_or_default()
        ));
    }
    res.json::<ChallengeJson>()
        .await
        .map_err(|e| format!("challenge json: {e}"))
}

fn build_proof(
    secret: &[u8],
    ch: &ChallengeJson,
    tamper: bool,
) -> Result<AuthAttemptJson, String> {
    let nonce_fixed = chal_auth_shared::nonce_from_base64_fixed(&ch.nonce)
        .map_err(|e| format!("nonce decode: {e}"))?;
    let mut tag = hmac_sha256_sign(secret, &nonce_fixed, ch.timestamp, &ch.session_id)
        .map_err(|e| format!("sign: {e}"))?;
    if tamper {
        tag[0] ^= 0x73;
    }
    let sig_hex = hex::encode(tag);
    Ok(AuthAttemptJson {
        session_id: ch.session_id.clone(),
        nonce: ch.nonce.clone(),
        timestamp: ch.timestamp,
        signature: sig_hex,
    })
}

async fn authenticate(
    cli: &reqwest::Client,
    base_url: &str,
    proof: AuthAttemptJson,
) -> Result<String, String> {
    let url = format!("{}/v1/authenticate", base(base_url));
    let res = cli
        .post(&url)
        .json(&proof)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        Ok(text)
    } else {
        let detail = serde_json::from_str::<AuthFailureJson>(&text)
            .ok()
            .map(|f| f.reason)
            .unwrap_or_else(|| text.clone());
        Err(format!("HTTP {status}: {detail}"))
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let secret = secret_from_hex(&cli.secret_hex)?;
    let http = reqwest::Client::builder()
        .build()
        .map_err(|e| e.to_string())?;

    match cli.cmd {
        Cmd::AuthenticateOk => {
            let ch = fetch_challenge(&http, &cli.base_url).await?;
            println!("challenge: {:?}", ch);
            let proof = build_proof(&secret, &ch, false)?;
            let body = authenticate(&http, &cli.base_url, proof).await?;
            let ok = serde_json::from_str::<AuthSuccessJson>(&body)
                .map_err(|_| format!("unexpected body: {}", body.trim()))?;
            println!("authenticated: {:?}", ok);
        }
        Cmd::AuthenticateTamperedMac => {
            let ch = fetch_challenge(&http, &cli.base_url).await?;
            println!("challenge: {:?}", ch);
            let proof = build_proof(&secret, &ch, true)?;
            match authenticate(&http, &cli.base_url, proof).await {
                Ok(b) => {
                    println!("unexpected success body: {}", b.trim());
                    return Err("expected auth failure".into());
                }
                Err(e) => println!("FAIL (expected tamper): {}", e),
            }
        }
        Cmd::ReplaySameProofTwice => {
            let ch = fetch_challenge(&http, &cli.base_url).await?;
            println!("challenge: {:?}", ch);
            let proof = build_proof(&secret, &ch, false)?;
            let first = authenticate(&http, &cli.base_url, proof.clone()).await?;
            println!("first: {}", first.trim());
            match authenticate(&http, &cli.base_url, proof).await {
                Ok(b) => {
                    println!("unexpected second success body: {}", b.trim());
                    return Err("replay should fail".into());
                }
                Err(e) => println!("second attempt FAIL (expected replay): {}", e),
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
