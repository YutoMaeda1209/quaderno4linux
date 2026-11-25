//! このクライアントと端末のペアリング (「登録」)。
//!
//! やり取りはポート 8080 の平文の HTTP で行う。
//!
//! 1. `PUT  /register/cleanup`
//! 2. `POST /register/pin`   → M1 `{a: N1, b: MAC, c: YB}` (端末に 8 桁の PIN が表示される)
//! 3. `POST /register/hash`  ← M2 `{a: N1, b: N2, c: MAC, d: YA, e: HMAC}` → M3 `{a: N2, b: eHash, e: HMAC}`
//! 4. `POST /register/ca`    ← M4 `{a: N1, b: rHash, d: wrap(RS), e: HMAC}` → M5 `{a: N2, d: wrap(ES||cert), e: HMAC}`
//! 5. `POST /register`       ← M6 `{a: N1, d: wrap(client_id||pubkey), e: HMAC}`
//! 6. `PUT  /register/cleanup`
//!
//! 手順 1〜3 は [`Registration::start`] で実行し、残りの手順は画面に表示された PIN をユーザーが
//! 入力したあと [`Registration::finish`] で実行する。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::blocking::Client;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::RsaPrivateKey;
use serde::Deserialize;
use serde_json::json;

use crate::address::DeviceAddress;
use crate::credentials::Credentials;
use crate::crypto::{normalize_dh, random_bytes, DiffieHellman, SessionKeys};
use crate::error::{Error, Result};
use crate::http::{build_client, check, http_url};

#[derive(Deserialize)]
struct Msg {
    #[serde(default)]
    a: Option<String>,
    #[serde(default)]
    b: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    d: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

fn field(v: &Option<String>, name: &str) -> Result<Vec<u8>> {
    let s = v.as_deref().ok_or_else(|| Error::Registration(format!("応答にフィールド {name} がありません")))?;
    B64.decode(s).map_err(|_| Error::Registration(format!("フィールド {name} の Base64 が不正です")))
}

fn b64(data: &[u8]) -> String {
    B64.encode(data)
}

/// 進行中のペアリング。端末は PIN を表示している。
pub struct Registration {
    http: Client,
    keys: SessionKeys,
    n1: Vec<u8>,
    n2: [u8; 16],
    ya: Vec<u8>,
    /// 受け取ったままの端末の公開鍵 (先頭に符号用の 0x00 が付いていることがある)。
    yb: Vec<u8>,
    e_hash: Vec<u8>,
    m3_hmac: Vec<u8>,
}

impl Registration {
    /// やり取りの前半を実行する。完了すると端末に PIN が表示される。
    pub fn start(addr: &DeviceAddress) -> Result<Self> {
        let http = build_client(addr, Some(std::time::Duration::from_secs(30)))?;
        check(http.put(http_url("/register/cleanup")).send()?)?;

        let m1: Msg = check(http.post(http_url("/register/pin")).send()?)?.json()?;
        let n1 = field(&m1.a, "a")?;
        let mac = field(&m1.b, "b")?;
        // YB は送られてきたとおりのバイト列で HMAC に入れる必要がある。端末は Java の
        // BigInteger.toByteArray() でシリアライズするので、先頭に 0x00 が付くことがある (257 バイト
        // )。256 バイトに詰め直すと、端末は 403 でやり取りを拒否する。
        let yb = field(&m1.c, "c")?;

        let n2: [u8; 16] = random_bytes();
        let dh = DiffieHellman::new();
        let mut ya = vec![0u8];
        ya.extend_from_slice(&dh.public_key());
        let zz = dh.shared_secret(&normalize_dh(&yb)?)?;

        let salt = [n1.as_slice(), &mac, &n2].concat();
        let keys = SessionKeys::derive(&zz, &salt);

        let m2_hmac = keys.hmac(&[&n1, &mac, &yb, &n1, &n2, &mac, &ya]);
        let m2 = json!({
            "a": b64(&n1), "b": b64(&n2), "c": b64(&mac), "d": b64(&ya), "e": b64(&m2_hmac),
        });
        let m3: Msg = check(http.post(http_url("/register/hash")).json(&m2).send()?)?.json()?;
        if field(&m3.a, "a")? != n2 {
            return Err(Error::Registration("ノンス N2 が一致しません".into()));
        }
        let e_hash = field(&m3.b, "b")?;
        let m3_hmac = field(&m3.e, "e")?;
        if keys.hmac(&[&n1, &n2, &mac, &ya, &m2_hmac, &n2, &e_hash])[..] != m3_hmac[..] {
            return Err(Error::Registration("M3 の HMAC が一致しません".into()));
        }

        Ok(Self { http, keys, n1, n2, ya, yb, e_hash, m3_hmac })
    }

    /// 端末に表示された PIN でやり取りを完了し、新しい認証情報を返す。
    pub fn finish(self, pin: &str) -> Result<Credentials> {
        let result = self.finish_inner(pin.trim());
        // 端末は必ずきれいな状態に戻しておく。
        let _ = self.http.put(http_url("/register/cleanup")).send();
        result
    }

    fn finish_inner(&self, pin: &str) -> Result<Credentials> {
        let Self { http, keys, n1, n2, ya, yb, e_hash, m3_hmac } = self;

        let psk = keys.hmac(&[pin.as_bytes()]);
        let rs: [u8; 16] = random_bytes();
        let r_hash = keys.hmac(&[&rs, &psk, yb, ya]);
        let wrapped_rs = keys.wrap(&rs);
        let m4_hmac = keys.hmac(&[n2, e_hash, m3_hmac, n1, &r_hash, &wrapped_rs]);
        let m4 = json!({
            "a": b64(n1), "b": b64(&r_hash), "d": b64(&wrapped_rs), "e": b64(&m4_hmac),
        });
        let resp = http.post(http_url("/register/ca")).json(&m4).send()?;
        let m5: Msg =
            check(resp).map_err(|e| Error::Registration(format!("PIN が正しくない可能性があります ({e})")))?.json()?;
        if field(&m5.a, "a")? != n2 {
            return Err(Error::Registration("ノンス N2 が一致しません".into()));
        }
        let wrapped_es_cert = field(&m5.d, "d")?;
        let m5_hmac = field(&m5.e, "e")?;
        if keys.hmac(&[n1, &r_hash, &wrapped_rs, &m4_hmac, n2, &wrapped_es_cert])[..] != m5_hmac[..] {
            return Err(Error::Registration("M5 の HMAC が一致しません".into()));
        }
        let es_cert = keys.unwrap(&wrapped_es_cert)?;
        if es_cert.len() < 16 {
            return Err(Error::Registration("証明書データが短すぎます".into()));
        }
        let (es, cert) = es_cert.split_at(16);
        if keys.hmac(&[es, &psk, yb, ya])[..] != e_hash[..] {
            return Err(Error::Registration("PIN が正しくありません".into()));
        }

        let private_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048)
            .map_err(|e| Error::Registration(format!("RSA 鍵の生成に失敗しました: {e}")))?;
        let public_pem = private_key
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| Error::Registration(e.to_string()))?;
        let public_pem = public_pem.trim_end();
        let client_id = uuid::Uuid::new_v4().to_string();

        let wrapped_did = keys.wrap(&[client_id.as_bytes(), public_pem.as_bytes()].concat());
        let m6_hmac = keys.hmac(&[n2, &wrapped_es_cert, &m5_hmac, n1, &wrapped_did]);
        let m6 = json!({ "a": b64(n1), "d": b64(&wrapped_did), "e": b64(&m6_hmac) });
        check(http.post(http_url("/register")).json(&m6).send()?)?;

        Ok(Credentials { client_id, private_key, certificate: Some(String::from_utf8_lossy(cert).into_owned()) })
    }
}
