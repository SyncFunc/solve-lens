use crate::AppState;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use p256::{ecdh::diffie_hellman, elliptic_curve::sec1::ToEncodedPoint, PublicKey, SecretKey};
use rand::RngCore;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Clone)]
struct CryptoState {
    sessions: Arc<Mutex<HashMap<String, ([u8; 32], Instant)>>>,
}
impl CryptoState {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn prune(&self) {
        let now = Instant::now();
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.retain(|_, (_, last_used)| now.duration_since(*last_used) <= SESSION_TTL);
        }
    }
    fn establish(&self, client_public: &[u8]) -> Result<(String, String), String> {
        self.prune();
        let client = PublicKey::from_sec1_bytes(client_public).map_err(|_| "无效的客户端公钥")?;
        let secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let public = secret.public_key().to_encoded_point(false);
        let shared = diffie_hellman(secret.to_nonzero_scalar(), client.as_affine());
        let digest = Sha256::digest(shared.raw_secret_bytes());
        let key: [u8; 32] = digest.into();
        let session_id = Uuid::new_v4().to_string();
        self.sessions
            .lock()
            .map_err(|_| "加密会话不可用")?
            .insert(session_id.clone(), (key, Instant::now()));
        Ok((session_id, STANDARD.encode(public.as_bytes())))
    }

    fn key(&self, session_id: &str) -> Option<[u8; 32]> {
        self.prune();
        let mut sessions = self.sessions.lock().ok()?;
        let (key, last_used) = sessions.get_mut(session_id)?;
        *last_used = Instant::now();
        Some(*key)
    }
}

#[derive(serde::Deserialize)]
struct HandshakeRequest {
    #[serde(rename = "clientPublic")]
    client_public: String,
}

#[derive(serde::Deserialize)]
struct EncryptedEnvelope {
    encrypted: Option<bool>,
    iv: String,
    data: String,
}

fn encrypt_bytes(key: &[u8; 32], plaintext: &[u8]) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "加密密钥无效")?;
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut iv);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&iv), plaintext)
        .map_err(|_| "加密失败")?;
    Ok(serde_json::to_string(&json!({
        "encrypted": true,
        "iv": STANDARD.encode(iv),
        "data": STANDARD.encode(ciphertext),
    }))
    .map_err(|_| "加密响应序列化失败")?)
}

fn decrypt_bytes(key: &[u8; 32], payload: &[u8]) -> Result<Vec<u8>, String> {
    let envelope: EncryptedEnvelope =
        serde_json::from_slice(payload).map_err(|_| "请求不是有效的加密数据")?;
    if envelope.encrypted != Some(true) {
        return Err("请求未标记为加密数据".into());
    }
    let iv = STANDARD.decode(envelope.iv).map_err(|_| "加密随机数无效")?;
    let data = STANDARD.decode(envelope.data).map_err(|_| "加密内容无效")?;
    if iv.len() != 12 {
        return Err("加密随机数长度无效".into());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "解密密钥无效")?;
    cipher
        .decrypt(Nonce::from_slice(&iv), data.as_ref())
        .map_err(|_| "无法解密请求内容（会话可能已失效）".into())
}
pub struct LocalServer {
    stop: Arc<AtomicBool>,
}

impl LocalServer {
    pub fn start(app: AppHandle, state: Arc<AppState>, port: u16) -> std::io::Result<Self> {
        // LAN control is opt-in at the configuration layer. Once enabled,
        // bind all interfaces so a phone on the same network can connect.
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let crypto = Arc::new(CryptoState::new());
        let signal = stop.clone();
        thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let connection_app = app.clone();
                        let connection_state = state.clone();
                        let connection_crypto = crypto.clone();
                        thread::spawn(move || {
                            let _ = handle(
                                &mut stream,
                                &connection_app,
                                &connection_state,
                                &connection_crypto,
                            );
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(40))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { stop })
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn websocket_accept(key: &str) -> String {
    let mut data = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").into_bytes();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0)
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    ];
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ])
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1)
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e)
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes())
    }
    STANDARD.encode(out)
}
fn read_http_request(stream: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    stream.set_read_timeout(Some(Duration::from_secs(8)))?;
    let mut bytes = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "empty HTTP request",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request too large",
            ));
        }
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let content_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    if content_length > 8 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP body too large",
        ));
    }
    while bytes.len() - header_end < content_length {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated HTTP body",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let mut body = bytes[header_end..].to_vec();
    body.truncate(content_length);
    Ok((head, body))
}

fn header_value<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().find_map(|line| {
        let (header, value) = line.split_once(':')?;
        header.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

fn query_value(path: &str, name: &str) -> Option<String> {
    path.split_once('?')?.1.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == name).then_some(value.to_owned())
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-Baobao-Session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    stream.write_all(response.as_bytes())
}

fn write_ws_frame(stream: &mut TcpStream, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len();
    let mut frame = Vec::with_capacity(len + 10);
    frame.push(0x81);
    if len < 126 {
        frame.push(len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)
}

fn read_ws_commands(mut reader: TcpStream, app: AppHandle, key: [u8; 32]) {
    loop {
        let mut header = [0u8; 2];
        if reader.read_exact(&mut header).is_err() {
            break;
        }
        let opcode = header[0] & 0x0f;
        if opcode == 0x8 {
            break;
        }
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as usize;
        if length == 126 {
            let mut bytes = [0u8; 2];
            if reader.read_exact(&mut bytes).is_err() {
                break;
            }
            length = u16::from_be_bytes(bytes) as usize;
        } else if length == 127 {
            let mut bytes = [0u8; 8];
            if reader.read_exact(&mut bytes).is_err() {
                break;
            }
            let value = u64::from_be_bytes(bytes);
            if value > usize::MAX as u64 {
                break;
            }
            length = value as usize;
        }
        if length > 1024 * 1024 {
            break;
        }
        let mut mask = [0u8; 4];
        if masked && reader.read_exact(&mut mask).is_err() {
            break;
        }
        let mut payload = vec![0u8; length];
        if reader.read_exact(&mut payload).is_err() {
            break;
        }
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        if opcode != 0x1 {
            continue;
        }
        let Ok(decrypted) = decrypt_bytes(&key, &payload) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&decrypted) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("command") {
            continue;
        }
        let action = value
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let command = match action {
            "capture" | "submit" | "clear" | "cancel" => action,
            _ => continue,
        };
        let preset_id = value
            .get("presetId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("general")
            .to_owned();
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();
        crate::dispatch_remote_command(app.clone(), id, command.to_owned(), preset_id);
    }
}

fn handle(
    stream: &mut TcpStream,
    app: &AppHandle,
    state: &Arc<AppState>,
    crypto: &Arc<CryptoState>,
) -> std::io::Result<()> {
    let (request, body) = read_http_request(stream)?;
    let mut lines = request.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    if method == "OPTIONS" {
        return write_response(stream, "204 No Content", "text/plain; charset=utf-8", "");
    }

    if method == "POST" && route == "/api/handshake" {
        let request: HandshakeRequest = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => {
                return write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":\"无效的握手请求\"}",
                )
            }
        };
        let client_public = match STANDARD.decode(request.client_public) {
            Ok(value) => value,
            Err(_) => {
                return write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":\"客户端公钥编码无效\"}",
                )
            }
        };
        let (session_id, server_public) = match crypto.establish(&client_public) {
            Ok(value) => value,
            Err(error) => {
                return write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    &serde_json::to_string(&json!({"error": error}))
                        .unwrap_or_else(|_| "{\"error\":\"握手失败\"}".into()),
                )
            }
        };
        let response =
            serde_json::to_string(&json!({"sessionId": session_id, "serverPublic": server_public}))
                .unwrap_or_else(|_| "{}".into());
        return write_response(
            stream,
            "200 OK",
            "application/json; charset=utf-8",
            &response,
        );
    }

    if route.starts_with("/assets/") && method == "GET" {
        let web_root = app
            .path()
            .resource_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("../dist"));
        let file = route.trim_start_matches("/assets/");
        let safe = !file.contains("..") && !file.contains('\\');
        if !safe {
            return write_response(
                stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                "Bad path",
            );
        }
        let path = web_root.join("assets").join(file);
        let bytes = std::fs::read(&path)
            .or_else(|_| std::fs::read(std::path::PathBuf::from("../dist/assets").join(file)));
        let Ok(bytes) = bytes else {
            return write_response(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                "Not found",
            );
        };
        let content_type = if file.ends_with(".js") {
            "application/javascript"
        } else if file.ends_with(".css") {
            "text/css"
        } else {
            "application/octet-stream"
        };
        let body = String::from_utf8_lossy(&bytes);
        return write_response(stream, "200 OK", content_type, &body);
    }
    if method == "GET" && route == "/" {
        let web_root = app
            .path()
            .resource_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("../dist"));
        let body = std::fs::read_to_string(web_root.join("mobile.html"))
            .or_else(|_| std::fs::read_to_string("../dist/mobile.html"))
            .unwrap_or_default();
        return write_response(stream, "200 OK", "text/html; charset=utf-8", &body);
    }

    let session_id = header_value(&request, "X-Baobao-Session")
        .map(str::to_owned)
        .or_else(|| query_value(path, "session"));
    let Some(session_id) = session_id else {
        return write_response(
            stream,
            "401 Unauthorized",
            "application/json",
            "{\"error\":\"需要先建立加密会话\"}",
        );
    };
    let Some(session_key) = crypto.key(&session_id) else {
        return write_response(
            stream,
            "401 Unauthorized",
            "application/json",
            "{\"error\":\"加密会话已失效，请刷新手机页面\"}",
        );
    };

    if method == "GET" && route == "/api/state" {
        let plaintext = serde_json::to_vec(&state.snapshot()).unwrap_or_else(|_| b"{}".to_vec());
        let encrypted = encrypt_bytes(&session_key, &plaintext).unwrap_or_else(|_| "{}".into());
        return write_response(
            stream,
            "200 OK",
            "application/json; charset=utf-8",
            &encrypted,
        );
    }
    if method == "GET"
        && route == "/ws"
        && request.to_ascii_lowercase().contains("upgrade: websocket")
    {
        let Some(key) = header_value(&request, "sec-websocket-key") else {
            return write_response(
                stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                "Missing WebSocket key",
            );
        };
        let accept = websocket_accept(key);
        stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes())?;
        stream.set_read_timeout(None)?;
        if let Ok(reader) = stream.try_clone() {
            let command_app = app.clone();
            thread::spawn(move || read_ws_commands(reader, command_app, session_key));
        }
        let mut last_plaintext = Vec::new();
        let mut last_ping = Instant::now();
        loop {
            let packet = json!({"type":"state.snapshot", "snapshot": state.snapshot()});
            let plaintext = serde_json::to_vec(&packet).unwrap_or_else(|_| b"{}".to_vec());
            if plaintext != last_plaintext {
                let encrypted = match encrypt_bytes(&session_key, &plaintext) {
                    Ok(value) => value,
                    Err(_) => break,
                };
                if write_ws_frame(stream, encrypted.as_bytes()).is_err() {
                    break;
                }
                last_plaintext = plaintext;
            }
            if last_ping.elapsed() >= Duration::from_secs(15) {
                if stream.write_all(&[0x89, 0]).is_err() {
                    break;
                }
                last_ping = Instant::now();
            }
            thread::sleep(Duration::from_millis(350));
        }
        return Ok(());
    }
    if method == "POST"
        && ["/api/capture", "/api/submit", "/api/clear", "/api/cancel"].contains(&route)
    {
        let decrypted = match decrypt_bytes(&session_key, &body) {
            Ok(value) => value,
            Err(error) => {
                return write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    &serde_json::to_string(&json!({"error": error}))
                        .unwrap_or_else(|_| "{\"error\":\"请求解密失败\"}".into()),
                )
            }
        };
        let value: serde_json::Value = match serde_json::from_slice(&decrypted) {
            Ok(value) => value,
            Err(_) => {
                return write_response(
                    stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":\"解密后的命令无效\"}",
                )
            }
        };
        let command = match route {
            "/api/capture" => "capture",
            "/api/submit" => "submit",
            "/api/clear" => "clear",
            _ => "cancel",
        };
        let command_id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let preset = value
            .get("presetId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("general");
        crate::dispatch_remote_command(
            app.clone(),
            command_id.to_owned(),
            command.to_owned(),
            preset.to_owned(),
        );
        let response = encrypt_bytes(
            &session_key,
            serde_json::to_string(&json!({"accepted": true, "id": command_id}))
                .unwrap_or_else(|_| "{}".into())
                .as_bytes(),
        )
        .unwrap_or_else(|_| "{}".into());
        return write_response(
            stream,
            "202 Accepted",
            "application/json; charset=utf-8",
            &response,
        );
    }
    write_response(
        stream,
        "404 Not Found",
        "text/plain; charset=utf-8",
        "Not found",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_gcm_round_trip_and_tamper_detection() {
        let key = [0x42u8; 32];
        let plaintext = br#"{"message":"baobao","count":2}"#;
        let envelope = encrypt_bytes(&key, plaintext).expect("encrypt");
        let decrypted = decrypt_bytes(&key, envelope.as_bytes()).expect("decrypt");
        assert_eq!(decrypted, plaintext);

        let mut tampered = serde_json::from_str::<serde_json::Value>(&envelope).expect("envelope");
        tampered["data"] = serde_json::Value::String("AAAA".into());
        assert!(decrypt_bytes(&key, tampered.to_string().as_bytes()).is_err());
    }

    #[test]
    fn ecdh_session_matches_browser_style_shared_x_coordinate() {
        let crypto = CryptoState::new();
        let client_secret = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let client_public = client_secret.public_key().to_encoded_point(false);
        let (session_id, server_public_b64) = crypto
            .establish(client_public.as_bytes())
            .expect("handshake");
        let server_public =
            PublicKey::from_sec1_bytes(&STANDARD.decode(server_public_b64).expect("base64"))
                .expect("server key");
        let shared = diffie_hellman(client_secret.to_nonzero_scalar(), server_public.as_affine());
        let expected: [u8; 32] = Sha256::digest(shared.raw_secret_bytes()).into();
        assert_eq!(crypto.key(&session_id), Some(expected));
    }
}
