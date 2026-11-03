//! 系统凭据存储。凭据以稳定的账户 ID 索引，不进入数据库或返回给前端。
//!
//! Windows 凭据管理器每条凭据最多保存 1280 个字符，而 OAuth 刷新令牌可能更长：
//! 超长的值拆成多段保存，主条目只记录段数。密码不允许包含控制字符，
//! 因此以控制字符开头的标记不会与真实密码混淆。

const SERVICE_NAME: &str = "com.wispmail.app";
/// 每段的字符数，留足 Windows 1280 字符上限的余量。
const CHUNK_CHARS: usize = 1000;
const CHUNK_MARKER: &str = "\u{1}wispmail-chunks:";
/// 最多分段数；同时用于清理旧值留下的多余分段。
const MAX_CHUNKS: usize = 16;

pub trait CredentialStore: Send {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, password: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

pub struct SystemCredentials;

fn entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE_NAME, id)
        .map_err(|_| "无法访问系统凭据管理器，请确认系统密钥存储可用。".into())
}

fn chunk_id(id: &str, index: usize) -> String {
    format!("{id}::part{index}")
}

/// 拆分超长的值；不需要拆分时返回 None。
pub(crate) fn split_secret(value: &str) -> Option<Vec<String>> {
    if value.chars().count() <= CHUNK_CHARS {
        return None;
    }
    let chars: Vec<char> = value.chars().collect();
    Some(
        chars
            .chunks(CHUNK_CHARS)
            .map(|chunk| chunk.iter().collect())
            .collect(),
    )
}

pub(crate) fn chunk_count(stored: &str) -> Option<usize> {
    stored
        .strip_prefix(CHUNK_MARKER)?
        .parse()
        .ok()
        .filter(|count| (1..=MAX_CHUNKS).contains(count))
}

fn read(id: &str) -> Result<Option<String>, String> {
    match entry(id)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("无法读取系统保存的凭据，请解锁系统凭据管理器后重试。".into()),
    }
}

fn write(id: &str, value: &str) -> Result<(), String> {
    entry(id)?
        .set_password(value)
        .map_err(|_| "无法将凭据保存到系统凭据管理器，账户尚未保存。".into())
}

fn remove(id: &str) -> Result<(), String> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("无法删除系统保存的凭据，账户尚未删除。".into()),
    }
}

fn remove_chunks_from(id: &str, first: usize) -> Result<(), String> {
    for index in first..MAX_CHUNKS {
        remove(&chunk_id(id, index))?;
    }
    Ok(())
}

impl CredentialStore for SystemCredentials {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        let Some(stored) = read(id)? else {
            return Ok(None);
        };
        let Some(count) = chunk_count(&stored) else {
            return Ok(Some(stored));
        };
        let mut value = String::new();
        for index in 0..count {
            value.push_str(
                &read(&chunk_id(id, index))?
                    .ok_or("系统保存的凭据不完整，请在账户设置中重新登录或填写密码。")?,
            );
        }
        Ok(Some(value))
    }

    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        let Some(chunks) = split_secret(password) else {
            write(id, password)?;
            return remove_chunks_from(id, 0);
        };
        if chunks.len() > MAX_CHUNKS {
            return Err("凭据过长，无法保存到系统凭据管理器。".into());
        }
        for (index, chunk) in chunks.iter().enumerate() {
            write(&chunk_id(id, index), chunk)?;
        }
        write(id, &format!("{CHUNK_MARKER}{}", chunks.len()))?;
        remove_chunks_from(id, chunks.len())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        remove(id)?;
        remove_chunks_from(id, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_secrets_are_split_and_short_ones_are_not() {
        assert!(split_secret("short-password").is_none());
        let long: String = "令".repeat(CHUNK_CHARS) + &"x".repeat(1500);
        let chunks = split_secret(&long).unwrap();
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= CHUNK_CHARS)
        );
        assert_eq!(chunks.concat(), long);
        assert_eq!(chunk_count(&format!("{CHUNK_MARKER}3")), Some(3));
        assert_eq!(chunk_count(&format!("{CHUNK_MARKER}0")), None);
        assert_eq!(chunk_count(&format!("{CHUNK_MARKER}99")), None);
        // 普通密码不可能以控制字符开头，不会被误认为分段标记。
        assert_eq!(chunk_count("wispmail-chunks:3"), None);
    }
}
