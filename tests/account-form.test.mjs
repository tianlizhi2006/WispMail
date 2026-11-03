import assert from "node:assert/strict";
import test from "node:test";
import {
  accountInputForSubmission,
  authCodeSubmission,
  createAccountDraft,
  createDiscoveryGuard,
  initialModeFor,
  requiresOutgoingPassword,
  setPasswordSharing,
  shouldDiscoverConfig,
} from "../src/features/accounts/accountForm.ts";
import {
  authCodeProviderFor,
  emailForProvider,
  normalizeAuthCode,
  AUTH_CODE_PROVIDERS,
} from "../src/features/accounts/authCodeProviders.ts";

const storedAccount = {
  id: "existing",
  email: "owner@example.com",
  displayName: "Owner",
  incomingProtocol: "imap",
  incoming: { host: "mail.example.com", port: 1993, encryption: "tls" },
  outgoing: { host: "mail.example.com", port: 1587, encryption: "starttls" },
  auth: "password",
};

test("legacy accounts retain custom endpoints and default to email usernames and a shared password", () => {
  const draft = createAccountDraft(storedAccount);
  assert.equal(draft.incoming.host, "mail.example.com");
  assert.equal(draft.outgoing.host, draft.incoming.host);
  assert.equal(draft.incoming.port, 1993);
  assert.equal(draft.outgoing.port, 1587);
  assert.equal(draft.incomingUsername, "");
  assert.equal(draft.outgoingUsername, "");
  assert.equal(draft.useSamePassword, true);
  assert.equal(draft.password, "");
  draft.incoming.host = "changed.example.com";
  assert.equal(storedAccount.incoming.host, "mail.example.com");
});

test("new accounts and a switch from shared to separate require an SMTP password", () => {
  assert.equal(requiresOutgoingPassword(createAccountDraft()), false);
  assert.equal(
    requiresOutgoingPassword(setPasswordSharing(createAccountDraft(), false)),
    true,
  );
  const existing = createAccountDraft(storedAccount);
  assert.equal(
    requiresOutgoingPassword(
      setPasswordSharing(existing, false),
      storedAccount,
    ),
    true,
  );
});

test("editing a separate SMTP credential can leave the password blank to preserve the stored credential", () => {
  const account = {
    ...storedAccount,
    useSamePassword: false,
    outgoingUsername: "smtp-user",
  };
  const draft = createAccountDraft(account);
  assert.equal(draft.outgoingPassword, "");
  assert.equal(draft.outgoingUsername, "smtp-user");
  assert.equal(requiresOutgoingPassword(draft, account), false);
});

test("switching back to a shared password clears the hidden SMTP secret and does not submit it", () => {
  const separate = {
    ...createAccountDraft(),
    useSamePassword: false,
    outgoingPassword: "temporary-smtp-secret",
  };
  const shared = setPasswordSharing(separate, true);
  assert.equal(shared.outgoingPassword, "");
  assert.equal(
    accountInputForSubmission({
      ...shared,
      outgoingPassword: "stale-hidden-value",
    }).outgoingPassword,
    null,
  );
  assert.equal(setPasswordSharing(shared, false).outgoingPassword, "");
});

test("domain usernames, distinct SMTP credentials, same hosts and custom ports survive submission", () => {
  const draft = {
    ...createAccountDraft(storedAccount),
    incomingUsername: " DOMAIN\\reader ",
    outgoingUsername: " smtp-service ",
    useSamePassword: false,
    password: " imap secret ",
    outgoingPassword: " smtp secret ",
  };
  const input = accountInputForSubmission(draft);
  assert.equal(input.incomingUsername, "DOMAIN\\reader");
  assert.equal(input.outgoingUsername, "smtp-service");
  assert.equal(input.password, " imap secret ");
  assert.equal(input.outgoingPassword, " smtp secret ");
  assert.equal(input.incoming.port, 1993);
  assert.equal(input.outgoing.port, 1587);
});

test("manual changes and configuration mode changes invalidate pending discovery", () => {
  const guard = createDiscoveryGuard();
  const automatic = guard.begin();
  assert.equal(guard.canApply(automatic), true);
  guard.invalidate();
  assert.equal(guard.canApply(automatic), false);
  const resumed = guard.begin();
  assert.equal(guard.canApply(resumed), true);
  assert.equal(guard.isLatest(automatic), false);
  const newerEmail = guard.begin();
  assert.equal(guard.canApply(resumed), false);
  assert.equal(guard.canApply(newerEmail), true);
});

test("manual mode never starts guessed discovery, even when the email changes or discovery is forced", () => {
  assert.equal(
    shouldDiscoverConfig("manual", "new@example.com", "old@example.com"),
    false,
  );
  assert.equal(
    shouldDiscoverConfig("manual", "new@example.com", "", true),
    false,
  );
  assert.equal(
    shouldDiscoverConfig("automatic", "new@example.com", "old@example.com"),
    true,
  );
  assert.equal(
    shouldDiscoverConfig("automatic", "new@example.com", "new@example.com"),
    false,
  );
  assert.equal(
    shouldDiscoverConfig(
      "automatic",
      "new@example.com",
      "new@example.com",
      true,
    ),
    true,
  );
});

test("QQ and NetEase addresses are recognised as authorization-code mailboxes", () => {
  assert.equal(authCodeProviderFor("Someone@QQ.com ")?.id, "qq");
  assert.equal(authCodeProviderFor("a@foxmail.com")?.id, "qq");
  assert.equal(authCodeProviderFor("a@163.com")?.id, "163");
  assert.equal(authCodeProviderFor("a@126.com")?.id, "126");
  assert.equal(authCodeProviderFor("a@yeah.net")?.id, "yeah");
  assert.equal(authCodeProviderFor("a@example.com"), undefined);
  assert.equal(authCodeProviderFor("no-at-sign"), undefined);
  const netease = AUTH_CODE_PROVIDERS.find((provider) => provider.id === "163");
  assert.equal(emailForProvider("zhang@qq.com", netease), "zhang@163.com");
  assert.equal(emailForProvider("zhang", netease), "zhang@163.com");
  assert.equal(emailForProvider("", netease), "", "nothing typed yet");
});

test("authorization codes lose pasted whitespace and always use one shared code with the email login", () => {
  assert.equal(
    normalizeAuthCode(" abcd efgh\nijkl\tmnop "),
    "abcdefghijklmnop",
  );
  const draft = {
    ...createAccountDraft(),
    email: " me@qq.com ",
    incoming: { host: "imap.qq.com", port: 993, encryption: "tls" },
    outgoing: { host: "smtp.qq.com", port: 465, encryption: "tls" },
    incomingUsername: "leftover",
    outgoingUsername: "leftover",
    useSamePassword: false,
    password: "abcd efgh ijkl mnop",
    outgoingPassword: "should-not-be-sent",
  };
  const input = authCodeSubmission(draft);
  assert.equal(input.email, "me@qq.com");
  assert.equal(input.password, "abcdefghijklmnop");
  assert.equal(input.useSamePassword, true);
  assert.equal(input.outgoingPassword, null);
  assert.equal(input.incomingUsername, "");
  assert.equal(input.outgoingUsername, "");
  assert.equal(
    authCodeSubmission({ ...draft, password: "" }).password,
    "",
    "editing keeps the stored code",
  );
});

test("authorization-code mode discovers servers and existing auth-code accounts reopen in it", () => {
  assert.equal(shouldDiscoverConfig("authcode", "me@163.com", ""), true);
  assert.equal(
    shouldDiscoverConfig("authcode", "me@163.com", "me@163.com"),
    false,
  );
  const qq = {
    ...storedAccount,
    email: "me@qq.com",
    incomingUsername: "",
    outgoingUsername: "",
    useSamePassword: true,
  };
  assert.equal(initialModeFor(qq), "authcode");
  assert.equal(
    initialModeFor({ ...qq, incomingUsername: "custom" }),
    "manual",
    "custom logins keep the full form",
  );
  assert.equal(initialModeFor({ ...qq, email: "me@example.com" }), "manual");
  assert.equal(initialModeFor(undefined), undefined);
});
