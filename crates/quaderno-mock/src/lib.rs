//! シミュレートした Quaderno 端末。
//!
//! ペアリングプロトコル (ポート 8080) と認証済みの文書 API (ポート 8443、自己署名の TLS) の端末側を、
//! メモリ上のファイルツリーで実装する。結合テストと、実機なしでの UI 開発
//! (`cargo run -p quaderno-mock`) に使う。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use quaderno_core::ROOT_FOLDER_ID;
use quaderno_core::address::{HTTP_PORT, HTTPS_PORT};
use quaderno_core::crypto::{DiffieHellman, SessionKeys, normalize_dh, random_bytes};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tiny_http::{Header, Method, Request, Response, Server};

type Resp = Response<std::io::Cursor<Vec<u8>>>;

#[derive(Clone)]
enum Kind {
    Folder,
    Document(Vec<u8>),
    /// ファイルのアップロード前に `POST /documents2` で作られる。フレームは一覧に表示されず、ファイ
    /// ルがアップロードされてはじめて名前が競合する。
    Frame,
}

#[derive(Clone)]
struct Node {
    id: String,
    name: String,
    parent: Option<String>,
    kind: Kind,
    created: String,
    modified: String,
}

/// `/register/pin` から `/register` までのペアリングの状態。
struct Pairing {
    dh: DiffieHellman,
    n1: [u8; 16],
    mac: [u8; 6],
    /// 送信したままの公開鍵 (Java の `BigInteger.toByteArray()` の形式)。
    yb: Vec<u8>,
    keys: Option<SessionKeys>,
    n2: Vec<u8>,
    ya: Vec<u8>,
    psk: [u8; 32],
    es: [u8; 16],
    e_hash: [u8; 32],
    m3_hmac: [u8; 32],
    r_hash: Vec<u8>,
    wrapped_es_cert: Vec<u8>,
    m5_hmac: [u8; 32],
}

struct State {
    pin: String,
    pairing: Option<Pairing>,
    clients: HashMap<String, RsaPublicKey>,
    nonces: HashMap<String, String>,
    sessions: Vec<String>,
    nodes: HashMap<String, Node>,
    next_id: u64,
}

/// 実行中のモック端末のハンドル。
pub struct MockDevice {
    state: Arc<Mutex<State>>,
    _threads: Vec<JoinHandle<()>>,
    pub ip: IpAddr,
}

impl MockDevice {
    /// `ip` で HTTP (8080) と HTTPS (8443) のサーバーを起動する。
    pub fn start(ip: IpAddr, pin: &str) -> Result<Self> {
        let root = Node::new(ROOT_FOLDER_ID.into(), "Document".into(), None, Kind::Folder);
        let nodes = HashMap::from([(root.id.clone(), root)]);
        let state = Arc::new(Mutex::new(State {
            pin: pin.into(),
            pairing: None,
            clients: HashMap::new(),
            nonces: HashMap::new(),
            sessions: Vec::new(),
            nodes,
            next_id: 1,
        }));

        let cert = rcgen::generate_simple_self_signed(vec!["quaderno.device".into()])?;
        let http = Server::http(SocketAddr::new(ip, HTTP_PORT)).map_err(|e| anyhow!("bind {HTTP_PORT}: {e}"))?;
        let https = Server::https(
            SocketAddr::new(ip, HTTPS_PORT),
            tiny_http::SslConfig {
                certificate: cert.cert.pem().into_bytes(),
                private_key: cert.key_pair.serialize_pem().into_bytes(),
            },
        )
        .map_err(|e| anyhow!("bind {HTTPS_PORT}: {e}"))?;

        let mut threads = Vec::new();
        for (server, secure) in [(Arc::new(http), false), (Arc::new(https), true)] {
            for _ in 0..4 {
                let server = server.clone();
                let state = state.clone();
                threads.push(std::thread::spawn(move || {
                    for req in server.incoming_requests() {
                        handle(&state, req, secure);
                    }
                }));
            }
        }
        Ok(Self { state, _threads: threads, ip })
    }

    /// 指定した内容の文書をフォルダ (既定はルート) に追加する。
    pub fn add_document(&self, parent: &str, name: &str, data: &[u8]) -> String {
        self.state.lock().unwrap().insert(parent.into(), name.into(), Kind::Document(data.to_vec()))
    }

    pub fn add_folder(&self, parent: &str, name: &str) -> String {
        self.state.lock().unwrap().insert(parent.into(), name.into(), Kind::Folder)
    }

    /// すべての文書のパスと内容の対応 (アサーション用)。
    pub fn documents(&self) -> HashMap<String, Vec<u8>> {
        let s = self.state.lock().unwrap();
        s.nodes
            .values()
            .filter_map(|n| match &n.kind {
                Kind::Document(d) => Some((s.path_of(&n.id), d.clone())),
                Kind::Folder | Kind::Frame => None,
            })
            .collect()
    }

    /// すべてのフォルダのパス (アサーション用)。
    pub fn folders(&self) -> Vec<String> {
        let s = self.state.lock().unwrap();
        let mut v: Vec<_> =
            s.nodes.values().filter(|n| matches!(n.kind, Kind::Folder)).map(|n| s.path_of(&n.id)).collect();
        v.sort();
        v
    }
}

fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // 日数から暦日への変換 (UTC)。表示用には十分。
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl Node {
    fn new(id: String, name: String, parent: Option<String>, kind: Kind) -> Self {
        let created = now();
        Self { id, name, parent, kind, modified: created.clone(), created }
    }
}

impl State {
    /// `parent` に新しい項目を追加し、その ID を返す。
    fn insert(&mut self, parent: String, name: String, kind: Kind) -> String {
        self.next_id += 1;
        let id = format!("{:08x}-mock-{}", self.next_id, uuid::Uuid::new_v4().simple());
        self.nodes.insert(id.clone(), Node::new(id.clone(), name, Some(parent), kind));
        id
    }

    fn path_of(&self, id: &str) -> String {
        let mut parts = Vec::new();
        let mut cur = self.nodes.get(id);
        while let Some(n) = cur {
            parts.push(n.name.clone());
            cur = n.parent.as_ref().and_then(|p| self.nodes.get(p));
        }
        parts.reverse();
        parts.join("/")
    }

    fn entry_json(&self, n: &Node) -> Value {
        let mut v = json!({
            "entry_id": n.id,
            "entry_name": n.name,
            "entry_path": self.path_of(&n.id),
            "created_date": n.created,
            "is_new": "false",
        });
        if let Some(p) = &n.parent {
            v["parent_folder_id"] = json!(p);
        }
        match &n.kind {
            Kind::Folder => v["entry_type"] = json!("folder"),
            Kind::Frame => {
                v["entry_type"] = json!("document");
                v["file_size"] = json!("0");
            }
            Kind::Document(data) => {
                v["entry_type"] = json!("document");
                v["file_size"] = json!(data.len().to_string());
                v["modified_date"] = json!(n.modified);
                v["mime_type"] = json!("application/pdf");
                v["document_type"] = json!("normal");
                v["total_page"] = json!("1");
                v["title"] = json!(n.name.trim_end_matches(".pdf"));
            }
        }
        v
    }

    fn children(&self, id: &str) -> Vec<&Node> {
        self.nodes.values().filter(|n| n.parent.as_deref() == Some(id) && !matches!(n.kind, Kind::Frame)).collect()
    }

    fn name_taken(&self, parent: &str, name: &str, except: Option<&str>) -> bool {
        self.children(parent).iter().any(|n| n.name == name && Some(n.id.as_str()) != except)
    }

    fn is_folder(&self, id: &str) -> bool {
        matches!(self.nodes.get(id).map(|n| &n.kind), Some(Kind::Folder))
    }

    fn remove_recursive(&mut self, id: &str) {
        let kids: Vec<String> = self.children(id).iter().map(|n| n.id.clone()).collect();
        for k in kids {
            self.remove_recursive(&k);
        }
        self.nodes.remove(id);
    }

    fn is_descendant(&self, id: &str, ancestor: &str) -> bool {
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            if c == ancestor {
                return true;
            }
            cur = self.nodes.get(&c).and_then(|n| n.parent.clone());
        }
        false
    }
}

fn err(status: u16, code: &str, message: &str) -> Resp {
    json_resp(status, json!({ "error_code": code, "message": message }))
}

fn json_resp(status: u16, v: Value) -> Resp {
    Response::from_data(serde_json::to_vec(&v).unwrap())
        .with_status_code(status)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn no_content() -> Resp {
    Response::from_data(Vec::new()).with_status_code(204)
}

fn b64(d: &[u8]) -> String {
    B64.encode(d)
}

fn field(v: &Value, k: &str) -> Result<Vec<u8>> {
    let s = v.get(k).and_then(Value::as_str).with_context(|| format!("missing field {k}"))?;
    Ok(B64.decode(s)?)
}

fn handle(state: &Arc<Mutex<State>>, mut req: Request, secure: bool) {
    let mut body = Vec::new();
    let _ = req.as_reader().read_to_end(&mut body);
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|h| (h.field.as_str().as_str().to_ascii_lowercase(), h.value.as_str().to_string()))
        .collect();
    let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    let resp = if secure { api(state, &method, &path, &body, &header) } else { register(state, &method, &path, &body) }
        .unwrap_or_else(|e| err(400, "40001", &format!("{e:#}")));
    log::debug!("{method} {path} -> {}", resp.status_code().0);
    let _ = req.respond(resp);
}

fn register(state: &Arc<Mutex<State>>, method: &Method, path: &str, body: &[u8]) -> Result<Resp> {
    let mut s = state.lock().unwrap();
    let body: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(body)? };
    Ok(match (method, path) {
        (Method::Get, "/register/information") => {
            json_resp(200, json!({ "serial_number": "MOCK0001", "model_name": "FMVDP51 (mock)" }))
        }
        (Method::Get, "/api_version") => json_resp(200, json!({ "value": "2.1" })),
        (Method::Put, "/register/cleanup") => {
            s.pairing = None;
            no_content()
        }
        (Method::Post, "/register/pin") => {
            let dh = DiffieHellman::new();
            let yb = java_bytes(&dh.public_key());
            let p = Pairing {
                dh,
                n1: random_bytes(),
                mac: random_bytes(),
                yb,
                keys: None,
                n2: Vec::new(),
                ya: Vec::new(),
                psk: [0; 32],
                es: random_bytes(),
                e_hash: [0; 32],
                m3_hmac: [0; 32],
                r_hash: Vec::new(),
                wrapped_es_cert: Vec::new(),
                m5_hmac: [0; 32],
            };
            let resp = json!({ "a": b64(&p.n1), "b": b64(&p.mac), "c": b64(&p.yb) });
            println!("[mock] PIN: {}", s.pin);
            s.pairing = Some(p);
            json_resp(200, resp)
        }
        (Method::Post, "/register/hash") => {
            let pin = s.pin.clone();
            let p = s.pairing.as_mut().context("no pairing")?;
            let (n1, n2, mac, ya, m2) =
                (field(&body, "a")?, field(&body, "b")?, field(&body, "c")?, field(&body, "d")?, field(&body, "e")?);
            anyhow::ensure!(n1 == p.n1 && mac == p.mac, "nonce/mac mismatch");
            let zz = p.dh.shared_secret(&normalize_dh(&ya)?)?;
            let keys = SessionKeys::derive(&zz, &[n1.as_slice(), &mac, &n2].concat());
            // 端末と同じく、送られてきたとおりの YB に対して HMAC を計算する。
            if keys.hmac(&[&n1, &mac, &p.yb, &n1, &n2, &mac, &ya])[..] != m2[..] {
                return Ok(err(403, "40300", "Bad parameters for registration process"));
            }
            p.psk = keys.hmac(&[pin.as_bytes()]);
            p.e_hash = keys.hmac(&[&p.es, &p.psk, &p.yb, &ya]);
            p.m3_hmac = keys.hmac(&[&n1, &n2, &mac, &ya, &m2, &n2, &p.e_hash]);
            let resp = json!({ "a": b64(&n2), "b": b64(&p.e_hash), "e": b64(&p.m3_hmac) });
            p.keys = Some(keys);
            p.n2 = n2;
            p.ya = ya;
            json_resp(200, resp)
        }
        (Method::Post, "/register/ca") => {
            let p = s.pairing.as_mut().context("no pairing")?;
            let keys = p.keys.as_ref().context("no keys")?;
            let (n1, r_hash, wrapped_rs, m4) =
                (field(&body, "a")?, field(&body, "b")?, field(&body, "d")?, field(&body, "e")?);
            anyhow::ensure!(n1 == p.n1, "nonce mismatch");
            anyhow::ensure!(
                keys.hmac(&[&p.n2, &p.e_hash, &p.m3_hmac, &n1, &r_hash, &wrapped_rs])[..] == m4[..],
                "M4 HMAC mismatch"
            );
            let rs = keys.unwrap(&wrapped_rs)?;
            if keys.hmac(&[&rs, &p.psk, &p.yb, &p.ya])[..] != r_hash[..] {
                return Ok(err(401, "40101", "PIN mismatch"));
            }
            let cert = b"-----BEGIN CERTIFICATE-----\nTU9DSw==\n-----END CERTIFICATE-----\n";
            let wrapped = keys.wrap(&[p.es.as_slice(), cert].concat());
            let m5 = keys.hmac(&[&n1, &r_hash, &wrapped_rs, &m4, &p.n2, &wrapped]);
            let resp = json!({ "a": b64(&p.n2), "d": b64(&wrapped), "e": b64(&m5) });
            p.r_hash = r_hash;
            p.wrapped_es_cert = wrapped;
            p.m5_hmac = m5;
            json_resp(200, resp)
        }
        (Method::Post, "/register") => {
            let p = s.pairing.as_ref().context("no pairing")?;
            let keys = p.keys.as_ref().context("no keys")?;
            let (n1, wrapped, m6) = (field(&body, "a")?, field(&body, "d")?, field(&body, "e")?);
            anyhow::ensure!(
                keys.hmac(&[&p.n2, &p.wrapped_es_cert, &p.m5_hmac, &n1, &wrapped])[..] == m6[..],
                "M6 HMAC mismatch"
            );
            let data = String::from_utf8(keys.unwrap(&wrapped)?)?;
            anyhow::ensure!(data.len() > 36, "short client data");
            let (id, pem) = data.split_at(36);
            let key = RsaPublicKey::from_public_key_pem(pem).context("bad public key")?;
            s.clients.insert(id.to_string(), key);
            println!("[mock] registered client {id}");
            no_content()
        }
        _ => err(404, "40401", "not found"),
    })
}

fn segs(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .map(|s| urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_default())
        .collect()
}

fn api(
    state: &Arc<Mutex<State>>,
    method: &Method,
    path: &str,
    body: &[u8],
    header: &dyn Fn(&str) -> Option<String>,
) -> Result<Resp> {
    let mut s = state.lock().unwrap();
    let seg = segs(path);
    let seg: Vec<&str> = seg.iter().map(String::as_str).collect();

    // --- 認証 (クッキー不要) ---
    match (method, seg.as_slice()) {
        (Method::Get, ["auth", "nonce", id]) => {
            if !s.clients.contains_key(*id) {
                return Ok(err(404, "40401", "client not registered"));
            }
            let nonce = b64(&random_bytes::<16>());
            s.nonces.insert(id.to_string(), nonce.clone());
            return Ok(json_resp(200, json!({ "nonce": nonce })));
        }
        (Method::Put, ["auth"]) => {
            let v: Value = serde_json::from_slice(body)?;
            let id = v["client_id"].as_str().context("client_id")?;
            let sig = B64.decode(v["nonce_signed"].as_str().context("nonce_signed")?)?;
            let nonce = s.nonces.remove(id).context("no nonce")?;
            let key = s.clients.get(id).context("unknown client")?;
            if key.verify(Pkcs1v15Sign::new::<Sha256>(), &Sha256::digest(nonce.as_bytes()), &sig).is_err() {
                return Ok(err(401, "40102", "signature mismatch"));
            }
            let token = b64(&random_bytes::<24>());
            s.sessions.push(token.clone());
            return Ok(no_content().with_header(
                Header::from_bytes("Set-Cookie", format!("Credentials={token}; Path=/; HttpOnly")).unwrap(),
            ));
        }
        _ => {}
    }

    let authed = header("cookie")
        .and_then(|c| {
            c.split(';')
                .filter_map(|kv| kv.trim().split_once('='))
                .find(|(k, _)| *k == "Credentials")
                .map(|(_, v)| v.to_string())
        })
        .is_some_and(|t| s.sessions.contains(&t));
    if !authed {
        return Ok(err(401, "40100", "Authentication is required."));
    }

    let body_json = || -> Result<Value> { Ok(serde_json::from_slice(body)?) };

    Ok(match (method, seg.as_slice()) {
        (Method::Get, ["ping"]) => no_content(),
        (Method::Get, ["system", "status", "battery"]) => json_resp(
            200,
            json!({ "level": "87", "plugged": "not_plugged", "status": "discharging", "health": "good" }),
        ),
        (Method::Get, ["system", "status", "storage"]) => {
            json_resp(200, json!({ "capacity": "15000000000", "available": "12345678901" }))
        }
        (Method::Get, ["system", "status", "firmware_version"]) => json_resp(200, json!({ "value": "2.2.09.11030FP" })),
        (Method::Get, ["folders", id, "entries"]) | (Method::Get, ["folders", id, "entries2"]) => {
            if !s.is_folder(id) {
                return Ok(err(404, "40401", "folder not found"));
            }
            let list: Vec<Value> = s.children(id).iter().map(|n| s.entry_json(n)).collect();
            json_resp(200, json!({ "count": list.len().to_string(), "entry_list": list }))
        }
        (Method::Get, ["resolve", "entry", "path", p]) => {
            match s.nodes.values().find(|n| !matches!(n.kind, Kind::Frame) && s.path_of(&n.id) == *p) {
                Some(n) => json_resp(200, s.entry_json(n)),
                None => err(404, "40401", "not found"),
            }
        }
        (Method::Post, ["folders2"]) | (Method::Post, ["folders"]) => {
            let v = body_json()?;
            let name = v["folder_name"].as_str().context("folder_name")?.to_string();
            let parent = v["parent_folder_id"].as_str().unwrap_or(ROOT_FOLDER_ID).to_string();
            if !s.is_folder(&parent) {
                return Ok(err(404, "40401", "parent not found"));
            }
            if s.name_taken(&parent, &name, None) {
                return Ok(err(400, "40007", "folderName already exists"));
            }
            let id = s.insert(parent, name, Kind::Folder);
            json_resp(200, json!({ "folder_id": id }))
        }
        (Method::Put, ["folders", id]) | (Method::Put, ["documents", id]) => {
            let v = body_json()?;
            if !s.nodes.contains_key(*id) || *id == ROOT_FOLDER_ID {
                return Ok(err(404, "40401", "not found"));
            }
            let node = s.nodes[*id].clone();
            let name = v
                .get(if seg[0] == "folders" { "folder_name" } else { "file_name" })
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(node.name.clone());
            let parent =
                v.get("parent_folder_id").and_then(Value::as_str).map(str::to_owned).or(node.parent.clone()).unwrap();
            if !s.is_folder(&parent) || s.is_descendant(&parent, id) {
                return Ok(err(400, "40003", "invalid destination"));
            }
            if s.name_taken(&parent, &name, Some(id)) {
                return Ok(err(400, "40007", "name already exists"));
            }
            let n = s.nodes.get_mut(*id).unwrap();
            n.name = name;
            n.parent = Some(parent);
            n.modified = now();
            no_content()
        }
        (Method::Delete, ["folders", id]) | (Method::Delete, ["documents", id]) => {
            if !s.nodes.contains_key(*id) || *id == ROOT_FOLDER_ID {
                return Ok(err(404, "40401", "not found"));
            }
            s.remove_recursive(id);
            no_content()
        }
        (Method::Post, ["documents2"]) | (Method::Post, ["documents"]) => {
            let v = body_json()?;
            let name = v["file_name"].as_str().context("file_name")?.to_string();
            let parent = v["parent_folder_id"].as_str().unwrap_or(ROOT_FOLDER_ID).to_string();
            if !s.is_folder(&parent) {
                return Ok(err(404, "40401", "parent not found"));
            }
            // 新しいフレームは同じ名前の古いフレームを置き換える。実際の文書はファイルのアップロー
            // ド時にだけ競合する。
            s.nodes.retain(|_, n| {
                !(matches!(n.kind, Kind::Frame) && n.name == name && n.parent.as_deref() == Some(&parent))
            });
            let id = s.insert(parent, name, Kind::Frame);
            json_resp(200, json!({ "document_id": id }))
        }
        (Method::Put, ["documents", id, "file"]) => {
            let ct = header("content-type").unwrap_or_default();
            let data = parse_multipart(&ct, body).context("multipart")?;
            let len = data.len();
            let Some(node) = s.nodes.get(*id).cloned() else { return Ok(err(404, "40401", "not found")) };
            match node.kind {
                Kind::Folder => return Ok(err(404, "40401", "not found")),
                Kind::Frame if s.name_taken(node.parent.as_deref().unwrap_or(ROOT_FOLDER_ID), &node.name, Some(id)) => {
                    return Ok(err(400, "40007", "fileName already exists"));
                }
                _ => {}
            }
            let n = s.nodes.get_mut(*id).unwrap();
            n.kind = Kind::Document(data);
            n.modified = now();
            json_resp(
                200,
                json!({ "completed": "true", "received_bytes": len.to_string(), "current_bytes": len.to_string() }),
            )
        }
        (Method::Get, ["documents", id, "file"]) => match s.nodes.get(*id).map(|n| &n.kind) {
            Some(Kind::Document(d)) => Response::from_data(d.clone())
                .with_header(Header::from_bytes("Content-Type", "application/pdf").unwrap()),
            _ => err(404, "40401", "not found"),
        },
        (Method::Get, ["documents", id]) | (Method::Get, ["folders", id]) => match s.nodes.get(*id) {
            Some(n) => json_resp(200, s.entry_json(n)),
            None => err(404, "40401", "not found"),
        },
        (Method::Post, ["documents", id, "copy"]) => {
            let v = body_json()?;
            let Some(Node { kind: Kind::Document(data), name, .. }) = s.nodes.get(*id).cloned() else {
                return Ok(err(404, "40401", "not found"));
            };
            let parent = v["parent_folder_id"].as_str().context("parent_folder_id")?.to_string();
            let name = v.get("file_name").and_then(Value::as_str).map(str::to_owned).unwrap_or(name);
            if !s.is_folder(&parent) {
                return Ok(err(404, "40401", "parent not found"));
            }
            if s.name_taken(&parent, &name, None) {
                return Ok(err(400, "40007", "fileName already exists"));
            }
            let new_id = s.insert(parent, name, Kind::Document(data));
            json_resp(200, json!({ "document_id": new_id }))
        }
        _ => err(404, "40401", "not found"),
    })
}

fn parse_multipart(content_type: &str, body: &[u8]) -> Option<Vec<u8>> {
    let boundary = content_type.split("boundary=").nth(1)?.trim_matches('"');
    let delim = format!("--{boundary}");
    let start = find(body, delim.as_bytes())?;
    let after = start + delim.len();
    let header_end = find(&body[after..], b"\r\n\r\n")? + after + 4;
    let end = find(&body[header_end..], format!("\r\n{delim}").as_bytes())? + header_end;
    Some(body[header_end..end].to_vec())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Java の `BigInteger.toByteArray()` と同じ、最小限の 2 の補数のビッグエンディアン表現。最上位ビッ
/// トが立っていると 0x00 を先頭に付ける (ほとんどの鍵で 257 バイトになる)。
fn java_bytes(n: &[u8]) -> Vec<u8> {
    let start = n.iter().position(|&b| b != 0).unwrap_or(n.len() - 1);
    let mut out = Vec::with_capacity(n.len() + 1);
    if n[start] & 0x80 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&n[start..]);
    out
}
