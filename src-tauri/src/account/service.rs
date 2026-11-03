//! 账户配置与系统凭据之间的协调。调用方必须串行执行一次完整的验证和保存操作。

use std::path::Path;

use crate::storage::Store;

use super::{
    Account, AccountCredentials, AccountInput, AuthMethod,
    credentials::{CredentialStore, SystemCredentials},
    validation::validate_input,
};

pub struct AccountService<C: CredentialStore = SystemCredentials> {
    store: Store,
    credentials: C,
}

struct CredentialChange<'a> {
    id: String,
    previous: Option<String>,
    next: Option<&'a str>,
}

fn smtp_credential_id(id: &str) -> String {
    format!("{id}::smtp")
}

impl AccountService<SystemCredentials> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        Ok(Self::new(
            Store::open(path).map_err(|e| e.to_string())?,
            SystemCredentials,
        ))
    }
}

impl<C: CredentialStore> AccountService<C> {
    pub fn new(store: Store, credentials: C) -> Self {
        Self { store, credentials }
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>, String> {
        self.store.list_accounts().map_err(|e| e.to_string())
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub fn get_account(&self, id: &str) -> Result<Account, String> {
        self.store
            .get_account(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "账户不存在，可能已被删除，请刷新账户列表。".to_string())
    }

    /// 仅在实际连接时读取系统凭据；离线浏览缓存不要求密钥存储在线。
    pub fn load_connection(&self, id: &str) -> Result<(Account, AccountCredentials), String> {
        let account = self.get_account(id)?;
        self.resolve_input(AccountInput {
            id: Some(account.id),
            email: account.email,
            display_name: account.display_name,
            incoming_username: account.incoming_username,
            outgoing_username: account.outgoing_username,
            use_same_password: account.use_same_password,
            incoming_protocol: account.incoming_protocol,
            incoming: account.incoming,
            outgoing: account.outgoing,
            auth: account.auth,
            password: None,
            outgoing_password: None,
        })
    }

    /// 验证输入并读取待测试的密码，不改变任何已保存的数据。
    pub fn resolve_input(
        &self,
        mut input: AccountInput,
    ) -> Result<(Account, AccountCredentials), String> {
        validate_input(&mut input)?;
        let previous = match &input.id {
            Some(id) => Some(
                self.store
                    .get_account(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "账户不存在，可能已被删除，请刷新账户列表。".to_string())?,
            ),
            None => None,
        };
        let id = input.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.ensure_unique_email(&input.email, &id)?;
        let oauth = input.auth == AuthMethod::OAuth2;
        let incoming_password = match input.password.filter(|value| !value.is_empty()) {
            Some(value) => value,
            None => self.credentials.get(&id)?.ok_or_else(|| {
                if oauth {
                    "未找到该账户的登录授权，请在账户设置中重新登录。".to_string()
                } else {
                    "请输入 IMAP 密码或授权码；如果原凭据已丢失，请重新填写。".to_string()
                }
            })?,
        };
        let outgoing_password = if input.use_same_password {
            incoming_password.clone()
        } else {
            match input.outgoing_password.filter(|value| !value.is_empty()) {
                Some(value) => value,
                None if previous
                    .as_ref()
                    .is_some_and(|account| !account.use_same_password) =>
                {
                    self.credentials
                        .get(&smtp_credential_id(&id))?
                        .ok_or_else(|| {
                            "未找到已保存的 SMTP 凭据，请重新填写 SMTP 密码或授权码。".to_string()
                        })?
                }
                None => return Err("首次使用独立 SMTP 密码时，请填写 SMTP 密码或授权码。".into()),
            }
        };
        Ok((
            Account {
                id,
                email: input.email,
                display_name: input.display_name,
                incoming_username: input.incoming_username,
                outgoing_username: input.outgoing_username,
                use_same_password: input.use_same_password,
                incoming_protocol: input.incoming_protocol,
                incoming: input.incoming,
                outgoing: input.outgoing,
                auth: input.auth,
            },
            AccountCredentials {
                incoming_password,
                outgoing_password,
            },
        ))
    }

    fn ensure_unique_email(&self, email: &str, id: &str) -> Result<(), String> {
        if let Some(owner) = self.store.email_owner(email).map_err(|e| e.to_string())?
            && owner != id
        {
            return Err("此邮箱已添加，请在账户列表中编辑现有账户。".into());
        }
        Ok(())
    }

    /// 只接收已经通过 IMAP 和 SMTP 登录验证的配置。凭据写入失败时不写数据库；
    /// 数据库写入失败时恢复原凭据，避免一次失败的编辑覆盖原登录信息。
    pub fn save_verified_account(
        &self,
        account: Account,
        credentials: &AccountCredentials,
    ) -> Result<Account, String> {
        self.ensure_unique_email(&account.email, &account.id)?;
        let exists = self
            .store
            .get_account(&account.id)
            .map_err(|e| e.to_string())?
            .is_some();
        // 先读取两个键的旧值，再执行任何修改；旧账户继续使用原始 id 保存 IMAP 密码。
        let changes = self.credential_changes(
            &account.id,
            Some(&credentials.incoming_password),
            if account.use_same_password {
                None
            } else {
                Some(&credentials.outgoing_password)
            },
        )?;
        self.apply_changes(&changes)?;
        let write_result = if exists {
            self.store.update_account(&account)
        } else {
            self.store.insert_account(&account)
        };
        if let Err(error) = write_result {
            return Err(self.rollback_error(error.to_string(), &changes));
        }
        Ok(account)
    }

    /// 保存服务商轮换后的新刷新令牌。只用于已保存的 OAuth 账户。
    pub fn replace_oauth_token(&self, id: &str, refresh_token: &str) -> Result<(), String> {
        if self.get_account(id)?.auth != AuthMethod::OAuth2 {
            return Err("该账户不使用 OAuth 登录。".into());
        }
        self.credentials.set(id, refresh_token)
    }

    /// 删除本地账户与本机保存的凭据，不触碰服务器上的邮件。
    pub fn delete_account(&self, id: &str) -> Result<(), String> {
        self.store
            .get_account(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "账户不存在，可能已被删除。".to_string())?;
        let changes = self.credential_changes(id, None, None)?;
        self.apply_changes(&changes)?;
        if let Err(error) = self.store.delete_account(id) {
            return Err(self.rollback_error(error.to_string(), &changes));
        }
        Ok(())
    }

    fn credential_changes<'a>(
        &self,
        id: &str,
        incoming: Option<&'a str>,
        outgoing: Option<&'a str>,
    ) -> Result<Vec<CredentialChange<'a>>, String> {
        let mut changes = Vec::new();
        for (id, next) in [
            (id.to_string(), incoming),
            (smtp_credential_id(id), outgoing),
        ] {
            let previous = self.credentials.get(&id)?;
            if previous.as_deref() != next {
                changes.push(CredentialChange { id, previous, next });
            }
        }
        Ok(changes)
    }

    fn apply_changes(&self, changes: &[CredentialChange<'_>]) -> Result<(), String> {
        for (index, change) in changes.iter().enumerate() {
            if let Err(error) = self.write_credential(&change.id, change.next) {
                return Err(self.rollback_error(error, &changes[..index]));
            }
        }
        Ok(())
    }

    fn rollback_error(&self, error: String, changes: &[CredentialChange<'_>]) -> String {
        let mut failed = false;
        // 即使某个键恢复失败，也继续恢复另一个键，尽可能保全原有凭据。
        for change in changes.iter().rev() {
            if self
                .write_credential(&change.id, change.previous.as_deref())
                .is_err()
            {
                failed = true;
            }
        }
        if failed {
            format!("{error}；恢复系统凭据失败，请重新填写该账户的收发信密码后保存。")
        } else {
            error
        }
    }

    fn write_credential(&self, id: &str, password: Option<&str>) -> Result<(), String> {
        match password {
            Some(password) => self.credentials.set(id, password),
            None => self.credentials.delete(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;
    use crate::account::{AuthMethod, Encryption, IncomingProtocol, ServerConfig};

    #[derive(Default)]
    struct FakeCredentials {
        values: Mutex<HashMap<String, String>>,
        fail_set: bool,
        fail_delete: bool,
        fail_password: Option<String>,
        fail_get_id: Option<String>,
        fail_set_id: Option<String>,
        fail_delete_id: Option<String>,
    }

    impl CredentialStore for FakeCredentials {
        fn get(&self, id: &str) -> Result<Option<String>, String> {
            if self.fail_get_id.as_deref() == Some(id) {
                return Err("credential read failed".into());
            }
            Ok(self.values.lock().unwrap().get(id).cloned())
        }
        fn set(&self, id: &str, password: &str) -> Result<(), String> {
            if self.fail_set
                || self.fail_password.as_deref() == Some(password)
                || self.fail_set_id.as_deref() == Some(id)
            {
                return Err("credential write failed".into());
            }
            self.values
                .lock()
                .unwrap()
                .insert(id.into(), password.into());
            Ok(())
        }
        fn delete(&self, id: &str) -> Result<(), String> {
            if self.fail_delete || self.fail_delete_id.as_deref() == Some(id) {
                return Err("credential delete failed".into());
            }
            self.values.lock().unwrap().remove(id);
            Ok(())
        }
    }

    fn input() -> AccountInput {
        AccountInput {
            id: None,
            email: " Alice@EXAMPLE.com ".into(),
            display_name: " Alice ".into(),
            incoming_username: String::new(),
            outgoing_username: String::new(),
            use_same_password: true,
            incoming_protocol: IncomingProtocol::Imap,
            incoming: ServerConfig {
                host: " imap.example.com ".into(),
                port: 993,
                encryption: Encryption::Tls,
            },
            outgoing: ServerConfig {
                host: "smtp.example.com".into(),
                port: 587,
                encryption: Encryption::StartTls,
            },
            auth: AuthMethod::Password,
            password: Some("private-test-password".into()),
            outgoing_password: None,
        }
    }

    fn service() -> AccountService<FakeCredentials> {
        AccountService::new(Store::in_memory(), FakeCredentials::default())
    }

    fn add(service: &AccountService<FakeCredentials>) -> Account {
        let (account, password) = service.resolve_input(input()).unwrap();
        service.save_verified_account(account, &password).unwrap()
    }

    fn shared_credentials(password: &str) -> AccountCredentials {
        AccountCredentials {
            incoming_password: password.into(),
            outgoing_password: password.into(),
        }
    }

    fn separate_input() -> AccountInput {
        let mut input = input();
        input.incoming_username = " CORP\\Alice ".into();
        input.outgoing_username = "smtp-alice".into();
        input.use_same_password = false;
        input.outgoing_password = Some("smtp-test-password".into());
        input
    }

    fn add_separate(service: &AccountService<FakeCredentials>) -> Account {
        let (account, credentials) = service.resolve_input(separate_input()).unwrap();
        service
            .save_verified_account(account, &credentials)
            .unwrap()
    }

    fn saved_password(service: &AccountService<FakeCredentials>, id: &str) -> Option<String> {
        service.credentials.get(id).unwrap()
    }

    #[test]
    fn separate_logins_and_passwords_are_saved_without_serializing_secrets() {
        let service = service();
        let (account, credentials) = service.resolve_input(separate_input()).unwrap();
        assert_eq!(account.incoming_login(), "CORP\\Alice");
        assert_eq!(account.outgoing_login(), "smtp-alice");
        assert_eq!(credentials.incoming_password, "private-test-password");
        assert_eq!(credentials.outgoing_password, "smtp-test-password");
        service
            .save_verified_account(account.clone(), &credentials)
            .unwrap();
        assert_eq!(
            saved_password(&service, &account.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&account.id)).as_deref(),
            Some("smtp-test-password")
        );
        let stored = service.list_accounts().unwrap();
        assert_eq!(stored, vec![account]);
        let json = serde_json::to_string(&stored).unwrap();
        assert!(!json.contains(&credentials.incoming_password));
        assert!(!json.contains(&credentials.outgoing_password));
    }

    #[test]
    fn blank_edit_preserves_independent_passwords_and_updates_only_requested_secret() {
        let service = service();
        let original = add_separate(&service);
        let mut change = separate_input();
        change.id = Some(original.id.clone());
        change.password = None;
        change.outgoing_password = Some(String::new());
        let (_, credentials) = service.resolve_input(change.clone()).unwrap();
        assert_eq!(credentials.incoming_password, "private-test-password");
        assert_eq!(credentials.outgoing_password, "smtp-test-password");
        change.password = Some("new-imap-password".into());
        let (updated, credentials) = service.resolve_input(change).unwrap();
        service
            .save_verified_account(updated, &credentials)
            .unwrap();
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("new-imap-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("smtp-test-password")
        );
    }

    #[test]
    fn switching_to_separate_requires_explicit_password_even_with_a_stale_smtp_key() {
        let service = service();
        let mut new = separate_input();
        new.outgoing_password = None;
        assert!(service.resolve_input(new).is_err());
        let original = add(&service);
        service
            .credentials
            .set(&smtp_credential_id(&original.id), "stale-smtp-password")
            .unwrap();
        let mut change = separate_input();
        change.id = Some(original.id.clone());
        change.password = None;
        change.outgoing_password = None;
        assert!(service.resolve_input(change.clone()).is_err());
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        change.outgoing_password = Some("explicit-smtp-password".into());
        let (updated, credentials) = service.resolve_input(change).unwrap();
        service
            .save_verified_account(updated, &credentials)
            .unwrap();
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("explicit-smtp-password")
        );
    }

    #[test]
    fn switching_back_to_shared_removes_separate_secret_and_reuses_imap_password() {
        let service = service();
        let original = add_separate(&service);
        let mut change = input();
        change.id = Some(original.id.clone());
        change.password = None;
        let (updated, credentials) = service.resolve_input(change).unwrap();
        assert_eq!(credentials.outgoing_password, "private-test-password");
        service
            .save_verified_account(updated, &credentials)
            .unwrap();
        assert!(saved_password(&service, &smtp_credential_id(&original.id)).is_none());
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
    }

    #[test]
    fn smtp_write_failure_restores_imap_password_and_preserves_configuration() {
        let mut service = service();
        let original = add_separate(&service);
        service.credentials.fail_set_id = Some(smtp_credential_id(&original.id));
        let changed = AccountCredentials {
            incoming_password: "new-imap-password".into(),
            outgoing_password: "new-smtp-password".into(),
        };
        assert!(
            service
                .save_verified_account(original.clone(), &changed)
                .is_err()
        );
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("smtp-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn new_smtp_write_failure_removes_already_written_imap_secret() {
        let mut service = service();
        let (account, credentials) = service.resolve_input(separate_input()).unwrap();
        service.credentials.fail_set_id = Some(smtp_credential_id(&account.id));
        assert!(
            service
                .save_verified_account(account.clone(), &credentials)
                .is_err()
        );
        assert!(saved_password(&service, &account.id).is_none());
        assert!(service.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn smtp_read_failure_happens_before_any_credential_changes() {
        let mut service = service();
        let original = add(&service);
        service.credentials.fail_get_id = Some(smtp_credential_id(&original.id));
        assert!(
            service
                .save_verified_account(
                    original.clone(),
                    &shared_credentials("replacement-password")
                )
                .is_err()
        );
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn failed_shared_switch_restores_separate_smtp_secret() {
        let service = service();
        let original = add_separate(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_update BEFORE UPDATE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        let mut shared = original.clone();
        shared.use_same_password = true;
        assert!(
            service
                .save_verified_account(shared, &shared_credentials("new-imap-password"))
                .is_err()
        );
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("smtp-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn smtp_delete_failure_restores_imap_secret_before_returning() {
        let mut service = service();
        let original = add_separate(&service);
        service.credentials.fail_delete_id = Some(smtp_credential_id(&original.id));
        assert!(service.delete_account(&original.id).is_err());
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("smtp-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn failed_database_delete_restores_both_secrets() {
        let service = service();
        let original = add_separate(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        assert!(service.delete_account(&original.id).is_err());
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(
            saved_password(&service, &smtp_credential_id(&original.id)).as_deref(),
            Some("smtp-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn smtp_compensation_failure_still_restores_imap_secret() {
        let mut service = service();
        let original = add_separate(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_update BEFORE UPDATE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        service.credentials.fail_password = Some("smtp-test-password".into());
        let changed = AccountCredentials {
            incoming_password: "new-imap-password".into(),
            outgoing_password: "new-smtp-password".into(),
        };
        let error = service
            .save_verified_account(original.clone(), &changed)
            .unwrap_err();
        assert!(error.contains("恢复系统凭据失败"));
        assert!(!error.contains("smtp-test-password"));
        assert!(!error.contains("new-imap-password"));
        assert_eq!(
            saved_password(&service, &original.id).as_deref(),
            Some("private-test-password")
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn deleting_separate_account_removes_both_credential_entries() {
        let service = service();
        let original = add_separate(&service);
        service.delete_account(&original.id).unwrap();
        assert!(saved_password(&service, &original.id).is_none());
        assert!(saved_password(&service, &smtp_credential_id(&original.id)).is_none());
        assert!(service.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn resolves_without_persisting_then_saves_credentials_separately() {
        let service = service();
        let (account, password) = service.resolve_input(input()).unwrap();
        assert!(service.list_accounts().unwrap().is_empty());
        assert!(service.credentials.get(&account.id).unwrap().is_none());
        assert_eq!(account.email, "Alice@example.com");
        assert_eq!(account.display_name, "Alice");
        assert_eq!(account.incoming.host, "imap.example.com");
        service
            .save_verified_account(account.clone(), &password)
            .unwrap();
        assert_eq!(service.list_accounts().unwrap(), vec![account.clone()]);
        assert_eq!(
            service.credentials.get(&account.id).unwrap().as_deref(),
            Some(password.incoming_password.as_str())
        );
        let json = serde_json::to_string(&service.list_accounts().unwrap()).unwrap();
        assert!(!json.contains(&password.incoming_password));
        assert!(
            serde_json::to_value(&account)
                .unwrap()
                .get("password")
                .is_none()
        );
    }

    #[test]
    fn duplicate_email_is_rejected_case_insensitively() {
        let service = service();
        add(&service);
        let mut duplicate = input();
        duplicate.email = "alice@example.com".into();
        assert!(service.resolve_input(duplicate).is_err());
        assert_eq!(service.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn edit_preserves_id_and_reuses_password_when_left_blank() {
        let service = service();
        let original = add(&service);
        let mut change = input();
        change.id = Some(original.id.clone());
        change.display_name = "Updated".into();
        change.password = Some(String::new());
        let (updated, password) = service.resolve_input(change).unwrap();
        assert_eq!(password.incoming_password, "private-test-password");
        assert_eq!(password.outgoing_password, "private-test-password");
        assert_eq!(updated.id, original.id);
        service
            .save_verified_account(updated.clone(), &password)
            .unwrap();
        assert_eq!(service.list_accounts().unwrap(), vec![updated]);
    }

    #[test]
    fn delete_removes_both_configuration_and_credential() {
        let service = service();
        let account = add(&service);
        service.delete_account(&account.id).unwrap();
        assert!(service.list_accounts().unwrap().is_empty());
        assert!(service.credentials.get(&account.id).unwrap().is_none());
        assert!(service.delete_account(&account.id).is_err());
    }

    #[test]
    fn credential_failure_does_not_save_configuration() {
        let service = AccountService::new(
            Store::in_memory(),
            FakeCredentials {
                fail_set: true,
                ..Default::default()
            },
        );
        let (account, password) = service.resolve_input(input()).unwrap();
        assert!(service.save_verified_account(account, &password).is_err());
        assert!(service.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn failed_insert_removes_new_credential() {
        let service = service();
        service.store.execute_test_batch("CREATE TRIGGER deny_insert BEFORE INSERT ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        let (account, password) = service.resolve_input(input()).unwrap();
        let id = account.id.clone();
        assert!(service.save_verified_account(account, &password).is_err());
        assert!(service.list_accounts().unwrap().is_empty());
        assert!(service.credentials.get(&id).unwrap().is_none());
    }

    #[test]
    fn failed_update_restores_original_credential() {
        let service = service();
        let original = add(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_update BEFORE UPDATE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        let mut changed = original.clone();
        changed.display_name = "Changed".into();
        assert!(
            service
                .save_verified_account(changed, &shared_credentials("replacement-password"))
                .is_err()
        );
        assert_eq!(service.list_accounts().unwrap(), vec![original.clone()]);
        assert_eq!(
            service.credentials.get(&original.id).unwrap().as_deref(),
            Some("private-test-password")
        );
    }

    #[test]
    fn failed_delete_restores_credential() {
        let service = service();
        let original = add(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        assert!(service.delete_account(&original.id).is_err());
        assert_eq!(service.list_accounts().unwrap(), vec![original.clone()]);
        assert_eq!(
            service.credentials.get(&original.id).unwrap().as_deref(),
            Some("private-test-password")
        );
    }

    #[test]
    fn failed_compensation_reports_that_credentials_need_repair() {
        let mut service = service();
        let original = add(&service);
        service.store.execute_test_batch("CREATE TRIGGER deny_update BEFORE UPDATE ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        service.credentials.fail_password = Some("private-test-password".into());
        let error = service
            .save_verified_account(
                original.clone(),
                &shared_credentials("replacement-password"),
            )
            .unwrap_err();
        assert!(error.contains("恢复系统凭据失败"));
        assert!(!error.contains("private-test-password"));
        assert!(!error.contains("replacement-password"));
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn failed_insert_cleanup_is_reported_without_leaking_password() {
        let mut service = service();
        service.store.execute_test_batch("CREATE TRIGGER deny_insert BEFORE INSERT ON accounts BEGIN SELECT RAISE(ABORT, 'test failure'); END;");
        service.credentials.fail_delete = true;
        let (account, password) = service.resolve_input(input()).unwrap();
        let error = service
            .save_verified_account(account, &password)
            .unwrap_err();
        assert!(error.contains("恢复系统凭据失败"));
        assert!(!error.contains(&password.incoming_password));
        assert!(service.list_accounts().unwrap().is_empty());
    }

    #[test]
    fn credential_delete_failure_preserves_account() {
        let mut service = service();
        let original = add(&service);
        service.credentials.fail_delete = true;
        assert!(service.delete_account(&original.id).is_err());
        assert_eq!(service.list_accounts().unwrap(), vec![original]);
    }

    #[test]
    fn rejects_unsupported_or_unsafe_connection_settings() {
        let service = service();
        let mut unsafe_input = input();
        unsafe_input.incoming.encryption = Encryption::None;
        assert!(service.resolve_input(unsafe_input).is_err());
        let mut bad_host = input();
        bad_host.incoming.host = "https://imap.example.com/path".into();
        assert!(service.resolve_input(bad_host).is_err());
        let mut bad_port = input();
        bad_port.outgoing.port = 0;
        assert!(service.resolve_input(bad_port).is_err());
        let mut oauth = input();
        oauth.auth = AuthMethod::OAuth2;
        assert!(service.resolve_input(oauth).is_err());
        let mut pop3 = input();
        pop3.incoming_protocol = IncomingProtocol::Pop3;
        assert!(service.resolve_input(pop3).is_err());
    }

    fn oauth_input() -> AccountInput {
        let provider = &crate::oauth::MICROSOFT;
        AccountInput {
            email: "me@outlook.com".into(),
            incoming_username: "ignored".into(),
            use_same_password: false,
            incoming: provider.incoming(),
            outgoing: provider.outgoing(true),
            auth: AuthMethod::OAuth2,
            password: None,
            ..input()
        }
    }

    #[test]
    fn oauth_accounts_use_the_stored_refresh_token_and_fixed_servers() {
        let service = service();
        let error = service.resolve_input(oauth_input()).err().unwrap();
        assert!(error.contains("重新登录"), "{error}");
        let mut with_password = oauth_input();
        with_password.password = Some("typed-password".into());
        assert!(service.resolve_input(with_password).is_err());
        let mut elsewhere = oauth_input();
        elsewhere.outgoing.host = "smtp.evil.test".into();
        assert!(service.resolve_input(elsewhere).is_err());

        let account = Account {
            id: "oauth-account".into(),
            email: "me@outlook.com".into(),
            display_name: "Me".into(),
            incoming_username: "me@outlook.com".into(),
            outgoing_username: "me@outlook.com".into(),
            use_same_password: true,
            incoming_protocol: IncomingProtocol::Imap,
            incoming: crate::oauth::MICROSOFT.incoming(),
            outgoing: crate::oauth::MICROSOFT.outgoing(true),
            auth: AuthMethod::OAuth2,
        };
        service
            .save_verified_account(account.clone(), &shared_credentials("refresh-1"))
            .unwrap();
        let (loaded, credentials) = service.load_connection(&account.id).unwrap();
        assert_eq!(loaded, account);
        assert_eq!(credentials.incoming_password, "refresh-1");
        assert_eq!(credentials.outgoing_password, "refresh-1");
        // 编辑显示名称时不需要也不允许输入密码，原授权保留。
        let edit = AccountInput {
            id: Some(account.id.clone()),
            display_name: "New name".into(),
            ..oauth_input()
        };
        let (edited, credentials) = service.resolve_input(edit).unwrap();
        assert_eq!(edited.incoming_username, "me@outlook.com");
        assert!(edited.use_same_password);
        assert_eq!(credentials.incoming_password, "refresh-1");

        service
            .replace_oauth_token(&account.id, "refresh-2")
            .unwrap();
        assert_eq!(
            service
                .load_connection(&account.id)
                .unwrap()
                .1
                .incoming_password,
            "refresh-2"
        );
        let password_account = add(&service);
        assert!(
            service
                .replace_oauth_token(&password_account.id, "x")
                .is_err()
        );
    }

    #[test]
    fn missing_password_and_unknown_edit_are_rejected() {
        let service = service();
        let mut missing = input();
        missing.password = None;
        assert!(service.resolve_input(missing).is_err());
        let mut unknown = input();
        unknown.id = Some("does-not-exist".into());
        assert!(service.resolve_input(unknown).is_err());
    }

    #[test]
    fn configuration_survives_reopening_database_without_password_bytes() {
        let folder =
            std::env::temp_dir().join(format!("wispmail-storage-test-{}", uuid::Uuid::new_v4()));
        let database = folder.join("accounts.sqlite3");
        let account = {
            let service =
                AccountService::new(Store::open(&database).unwrap(), FakeCredentials::default());
            add_separate(&service)
        };
        {
            let reopened = Store::open(&database).unwrap();
            assert_eq!(reopened.list_accounts().unwrap(), vec![account]);
        }
        let bytes = std::fs::read(&database).unwrap();
        for secret in [
            b"private-test-password".as_slice(),
            b"smtp-test-password".as_slice(),
        ] {
            assert!(!bytes.windows(secret.len()).any(|value| value == secret));
        }
        std::fs::remove_file(database).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn legacy_database_reopens_and_reuses_original_key_without_migrating_credentials() {
        let folder =
            std::env::temp_dir().join(format!("wispmail-legacy-test-{}", uuid::Uuid::new_v4()));
        let database = folder.join("accounts.sqlite3");
        let service =
            AccountService::new(Store::open(&database).unwrap(), FakeCredentials::default());
        let original = add(&service);
        let mut legacy = serde_json::to_value(&original).unwrap();
        let fields = legacy.as_object_mut().unwrap();
        fields.remove("incomingUsername");
        fields.remove("outgoingUsername");
        fields.remove("useSamePassword");
        let legacy_json = serde_json::to_string(&legacy).unwrap();
        let credentials = service.credentials;
        drop(service.store);
        {
            let database = rusqlite::Connection::open(&database).unwrap();
            database
                .execute(
                    "UPDATE accounts SET configuration = ?1 WHERE id = ?2",
                    rusqlite::params![legacy_json, original.id],
                )
                .unwrap();
        }
        {
            let reopened = AccountService::new(Store::open(&database).unwrap(), credentials);
            let accounts = reopened.list_accounts().unwrap();
            assert_eq!(accounts, vec![original.clone()]);
            assert_eq!(accounts[0].incoming_login(), "Alice@example.com");
            assert!(accounts[0].use_same_password);
            let mut change = input();
            change.id = Some(original.id.clone());
            change.password = None;
            let (_, resolved) = reopened.resolve_input(change).unwrap();
            assert_eq!(resolved.incoming_password, "private-test-password");
            assert_eq!(resolved.outgoing_password, "private-test-password");
            assert_eq!(reopened.credentials.values.lock().unwrap().len(), 1);
            assert!(saved_password(&reopened, &smtp_credential_id(&original.id)).is_none());
            // 读取和测试旧账户不会重写数据库 JSON 或操作凭据键。
            let database = rusqlite::Connection::open(&database).unwrap();
            let stored: String = database
                .query_row(
                    "SELECT configuration FROM accounts WHERE id = ?1",
                    [&original.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored, legacy_json);
        }
        std::fs::remove_file(database).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
}
