import assert from "node:assert/strict";
import test from "node:test";
import {
  accountInputForSubmission,
  createAccountDraft,
  createDiscoveryGuard,
  requiresOutgoingPassword,
  setPasswordSharing,
  shouldDiscoverConfig,
} from "../src/features/accounts/accountForm.ts";

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
