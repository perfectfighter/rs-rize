use crate::{ApiError, ApiResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use scrypt::{scrypt, Params};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const SESSION_TTL: i64 = 7 * 86_400_000;
pub fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}
pub fn clean_text(value: &Value, name: &str, max: usize, allow_empty: bool) -> ApiResult<String> {
    let Some(text) = value.as_str() else {
        return Err(ApiError::bad(format!(
            "{name} must be text of at most {max} characters."
        )));
    };
    if text.encode_utf16().count() > max
        || text
            .chars()
            .any(|c| matches!(c as u32, 0..=8|11|12|14..=31|127))
    {
        return Err(ApiError::bad(format!(
            "{name} must be text of at most {max} characters."
        )));
    }
    let text = text.trim().to_string();
    if !allow_empty && text.is_empty() {
        return Err(ApiError::bad(format!("{name} is required.")));
    }
    Ok(text)
}
pub fn email(value: &Value) -> ApiResult<String> {
    let text = clean_text(value, "Email", 254, false)?.to_lowercase();
    let parts: Vec<_> = text.split('@').collect();
    if parts.len() != 2
        || parts[0].is_empty()
        || !parts[1].contains('.')
        || parts[1].starts_with('.')
        || parts[1].ends_with('.')
        || text.chars().any(char::is_whitespace)
    {
        return Err(ApiError::bad("Enter a valid email address."));
    }
    Ok(text)
}
pub fn password(value: &Value) -> ApiResult<String> {
    let text = value
        .as_str()
        .ok_or_else(|| ApiError::bad("Password must contain 12 to 256 characters."))?;
    if !(12..=256).contains(&text.encode_utf16().count()) {
        return Err(ApiError::bad("Password must contain 12 to 256 characters."));
    }
    Ok(text.to_string())
}
fn derive(secret: &str, salt: &str) -> ApiResult<Vec<u8>> {
    let mut output = vec![0; 64];
    let params = Params::new(15, 8, 3, 64).map_err(|_| ApiError::internal())?;
    scrypt(secret.as_bytes(), salt.as_bytes(), &params, &mut output)
        .map_err(|_| ApiError::internal())?;
    Ok(output)
}
pub fn password_hash(secret: &str) -> ApiResult<String> {
    let mut salt = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let salt: String = salt.iter().map(|b| format!("{b:02x}")).collect();
    let hash = derive(secret, &salt)?;
    Ok(format!(
        "scrypt:32768:8:3:{salt}:{}",
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
pub fn check_password(secret: &str, stored: Option<&str>) -> ApiResult<bool> {
    let dummy = format!("scrypt:32768:8:3:{}:{}", "a".repeat(32), "b".repeat(128));
    let parts: Vec<_> = stored.unwrap_or(&dummy).split(':').collect();
    if parts.len() != 6
        || parts[..4] != ["scrypt", "32768", "8", "3"]
        || parts[4].len() != 32
        || parts[5].len() != 128
        || !parts[4..].iter().all(|v| {
            v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Ok(false);
    }
    let expected: Vec<u8> = (0..128)
        .step_by(2)
        .map(|i| u8::from_str_radix(&parts[5][i..i + 2], 16).unwrap_or(0))
        .collect();
    let actual = derive(secret, parts[4])?;
    Ok(bool::from(actual.ct_eq(&expected)))
}
pub fn issue_session(db: &Connection, user_id: &str, now: i64) -> ApiResult<String> {
    let token = random_token(32);
    db.execute("DELETE FROM sessions WHERE expires_at<=?", [now])?;
    db.execute(
        "INSERT INTO sessions(token_hash,user_id,expires_at,created_at) VALUES(?,?,?,?)",
        params![digest(&token), user_id, now + SESSION_TTL, now],
    )?;
    Ok(token)
}
pub fn authenticated(db: &Connection, authorization: Option<&str>, now: i64) -> ApiResult<Value> {
    let token = authorization
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .ok_or_else(|| ApiError::new(401, "Sign in to continue.", "UNAUTHENTICATED"))?;
    let row=db.query_row("SELECT u.id,u.email,u.name,s.expires_at FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=?",[digest(token)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).optional()?;
    match row {
        Some((id, email, name, expiry)) if expiry > now => {
            Ok(json!({"id":id,"email":email,"name":name}))
        }
        _ => {
            db.execute("DELETE FROM sessions WHERE token_hash=?", [digest(token)])?;
            Err(ApiError::new(
                401,
                "Your session has expired. Sign in again.",
                "UNAUTHENTICATED",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_input() {
        assert!(email(&json!("a@b")).is_err());
        assert_eq!(email(&json!(" A@B.test ")).unwrap(), "a@b.test");
        assert!(password(&json!("short")).is_err());
    }
    #[test]
    fn scrypt_node_format() {
        /* Published Node v1.2 crypto.scrypt N32768/r8/p3 with ASCII hex salt. */
        let hash="scrypt:32768:8:3:0123456789abcdef0123456789abcdef:74673523cfdf733fa88460ab587537ac873be17896bd9ca92408b9790de5c493b7ae9a7506c04269aca28d8c20da28e5674d3add4c99fbb58aed7fbe54cf7767";
        assert!(check_password("synthetic cross-language password", Some(hash)).unwrap());
        assert!(!check_password("incorrect password", Some(hash)).unwrap());
        assert!(!check_password("synthetic passphrase", None).unwrap());
    }
}
