//! 系统凭据存储。凭据以稳定的账户 ID 索引，不进入数据库或返回给前端。

const SERVICE_NAME: &str = "com.wispmail.app";

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

impl CredentialStore for SystemCredentials {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match entry(id)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("无法读取系统保存的凭据，请解锁系统凭据管理器后重试。".into()),
        }
    }

    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        entry(id)?
            .set_password(password)
            .map_err(|_| "无法将凭据保存到系统凭据管理器，账户尚未保存。".into())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("无法删除系统保存的凭据，账户尚未删除。".into()),
        }
    }
}
