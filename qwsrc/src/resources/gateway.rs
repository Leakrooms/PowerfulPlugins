//! 轻腕 (qingwear.top) 加密网关客户端。
//!
//! 方案逆向自网页版 bundle（见 QW_API_findings.md §5）：
//! AES-256-CBC + PKCS7，硬编码静态密钥，传输体为 base64(IV ‖ 密文)。
//! 请求信封 `{p,m,b,t,n}` 加密后 `POST /api/v2/x`，body `{"d": <密文>}`；
//! 响应同样是 `{"d": <密文>}`，解密后为 `{code,message,data,success}`。
//! 公开浏览/下载无需鉴权。

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use serde_json::{Value, json};
use waki::Client;

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// 32 字节静态密钥 `qwov2024-api-encrypt-secret-key!`。
const KEY: &[u8; 32] = b"qwov2024-api-encrypt-secret-key!";
const GATEWAY_ORIGIN: &str = "https://qingwear.top";
const GATEWAY_PATH: &str = "/api/v2/x";
const REQUEST_TIMEOUT_SECS: u64 = 20;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// 生成 16 字节 IV。wasm 环境无 getrandom，用时间 + 自增计数器播种 ChaCha20 保证唯一性。
fn random_iv() -> [u8; 16] {
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let left = md5::compute(format!("{nanos}-{counter}-iv").as_bytes());
    let right = md5::compute(format!("{counter}-{nanos}-qw").as_bytes());
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(&left.0);
    seed[16..].copy_from_slice(&right.0);
    let mut rng = ChaCha20Rng::from_seed(seed);
    let mut iv = [0u8; 16];
    rng.fill_bytes(&mut iv);
    iv
}

/// uuid v4（仅依赖 ChaCha 随机源）。
fn uuid4() -> String {
    let mut bytes = [0u8; 16];
    let mut rng = ChaCha20Rng::from_seed({
        let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = now_ms();
        let left = md5::compute(format!("{nanos}-{counter}-uuid").as_bytes());
        let right = md5::compute(format!("{counter}-{nanos}-n").as_bytes());
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(&left.0);
        seed[16..].copy_from_slice(&right.0);
        seed
    });
    rng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = |slice: &[u8]| slice.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        h(&bytes[0..4]),
        h(&bytes[4..6]),
        h(&bytes[6..8]),
        h(&bytes[8..10]),
        h(&bytes[10..16])
    )
}

/// `qv(plain)`：IV = random(16)；ct = AES-256-CBC(plain)；输出 base64(IV ‖ ct)。
fn encrypt_envelope(plaintext: &str) -> Result<String> {
    let iv = random_iv();
    let cipher = Aes256CbcEnc::new_from_slices(KEY, &iv)
        .map_err(|err| anyhow!("invalid aes key/iv: {err}"))?;
    let ct = cipher.encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    let mut combined = Vec::with_capacity(iv.len() + ct.len());
    combined.extend_from_slice(&iv);
    combined.extend_from_slice(&ct);
    Ok(BASE64_STANDARD.encode(combined))
}

/// `Jv(b64)`：base64decode；IV=buf[0:16]，ct=buf[16:]；AES-256-CBC 解密。
fn decrypt_envelope(b64: &str) -> Result<String> {
    let buf = BASE64_STANDARD
        .decode(b64.trim())
        .with_context(|| "failed to base64-decode gateway payload")?;
    if buf.len() < 16 {
        return Err(anyhow!("gateway payload too short: {} bytes", buf.len()));
    }
    let (iv, ct) = buf.split_at(16);
    let cipher = Aes256CbcDec::new_from_slices(KEY, iv)
        .map_err(|err| anyhow!("invalid aes key/iv: {err}"))?;
    let pt = cipher
        .decrypt_padded_vec_mut::<Pkcs7>(ct)
        .map_err(|err| anyhow!("aes-cbc decrypt/unpad failed: {err}"))?;
    String::from_utf8(pt).with_context(|| "decrypted payload is not valid utf-8")
}

/// 通过加密网关调用一个逻辑接口，返回解密后的完整响应 JSON
/// （`{code,message,data,success}`）。
pub fn api(path: &str, method: &str, body: Option<Value>) -> Result<Value> {
    let method_upper = method.to_ascii_uppercase();
    let envelope = json!({
        "p": path,
        "m": method_upper,
        "b": body.unwrap_or(Value::Null),
        "t": now_ms(),
        "n": uuid4(),
    });
    let encrypted = encrypt_envelope(&serde_json::to_string(&envelope)?)?;
    let request_body = json!({ "d": encrypted }).to_string();
    let url = format!("{GATEWAY_ORIGIN}{GATEWAY_PATH}");

    tracing::info!("gateway request: path={path} method={method_upper}");
    let response = Client::new()
        .post(&url)
        .headers([
            ("Content-Type", "application/json"),
            ("X-App-Variant-Supported", "1"),
            ("X-Client-App", "qingwear_web"),
            ("Origin", GATEWAY_ORIGIN),
            ("Referer", "https://qingwear.top/explore"),
            (
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
            ),
        ])
        .body(request_body.into_bytes())
        .connect_timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .send()
        .with_context(|| format!("gateway request failed: {path}"))?;

    let status = response.status_code();
    let raw = response
        .body()
        .with_context(|| "failed to read gateway response body")?;
    let raw_str = String::from_utf8_lossy(&raw).to_string();
    tracing::info!("gateway response: path={path} status={status} body_len={}", raw.len());

    if status >= 400 {
        return Err(anyhow!("gateway http {status}: {raw_str}"));
    }

    let outer: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("gateway response not JSON: {raw_str}"))?;
    let decoded = match outer.get("d").and_then(Value::as_str) {
        Some(d) => serde_json::from_str::<Value>(&decrypt_envelope(d)?)
            .with_context(|| "decrypted gateway payload is not JSON")?,
        None => outer,
    };
    Ok(decoded)
}

/// 调用网关并校验业务 `code`/`success`，返回 `data` 字段。
pub fn api_data(path: &str, method: &str, body: Option<Value>) -> Result<Value> {
    let response = api(path, method, body)?;

    if let Some(code) = response.get("code").and_then(Value::as_i64) {
        if code != 200 && code != 0 {
            let message = response
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown gateway error");
            return Err(anyhow!("gateway business error {code}: {message}"));
        }
    }
    if let Some(false) = response.get("success").and_then(Value::as_bool) {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("request not successful");
        return Err(anyhow!("gateway returned success=false: {message}"));
    }

    Ok(response.get("data").cloned().unwrap_or(Value::Null))
}
