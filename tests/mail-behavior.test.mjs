import assert from "node:assert/strict";
import test from "node:test";
import {
  base64Size,
  createDraftWriter,
  createRequestGuard,
  emptyMessage,
  MAX_ATTACHMENT_BYTES,
  mergeMessagePages,
  parseAddresses,
  prepareSend,
  replyMessage,
  selectionRemainsInPage,
  splitAddresses,
} from "../src/features/messages/mailHelpers.ts";

const detail = {
  summary: {
    uid: 17,
    uidValidity: 4,
    subject: "Project notes",
    from: '"Doe, Jane" <jane@example.com>',
    to: ["me@example.com", "teammate@example.com"],
    date: "2026-11-02",
    unread: true,
    flagged: false,
    size: 300,
    snippet: "Hello",
    hasAttachments: false,
  },
  cc: ["copy@example.com", "ME@example.com", "teammate@example.com"],
  replyTo: ["reply@example.com"],
  messageId: "<original@example.com>",
  references: ["<thread@example.com>"],
  textBody: "First line\nSecond line",
  htmlBody: null,
  attachments: [],
};

test("address input accepts quoted display names and common separators without splitting quoted commas", () => {
  assert.deepEqual(
    splitAddresses(
      '"Doe, Jane" <jane@example.com>; a@example.com，b@example.com',
    ),
    ['"Doe, Jane" <jane@example.com>', "a@example.com", "b@example.com"],
  );
  assert.deepEqual(
    parseAddresses([
      '"Doe, Jane" <jane@example.com>; JANE@example.com',
      "other@example.com",
    ]),
    { addresses: ["jane@example.com", "other@example.com"], invalid: [] },
  );
});

test("drafts can retain incomplete addresses but send preparation rejects invalid or empty recipients", () => {
  const draft = {
    ...emptyMessage(),
    to: ["unfinished@"],
    subject: "",
    body: "",
  };
  assert.equal(draft.to[0], "unfinished@");
  assert.throws(() => prepareSend(draft), /地址尚未填写完整/);
  assert.throws(() => prepareSend(emptyMessage()), /至少填写一个/);
  assert.equal(
    prepareSend({ ...emptyMessage(), bcc: ["hidden@example.com"] }).bcc[0],
    "hidden@example.com",
  );
  assert.deepEqual(
    parseAddresses([
      ".bad@example.com",
      "bad..dot@example.com",
      "a@-example.com",
    ]).addresses,
    [],
  );
});

test("send normalization does not change body, reply headers or attachment bytes", () => {
  const draft = {
    ...emptyMessage(),
    to: ['"Reader" <reader@example.com>, second@example.com'],
    body: " exact body\n",
    inReplyTo: "<id@example.com>",
    references: ["<root@example.com>"],
    attachments: [
      { name: "file.txt", mimeType: "text/plain", dataBase64: "YQ==" },
    ],
  };
  const normalized = prepareSend(draft);
  assert.deepEqual(normalized.to, ["reader@example.com", "second@example.com"]);
  assert.equal(normalized.body, draft.body);
  assert.equal(normalized.inReplyTo, draft.inReplyTo);
  assert.deepEqual(normalized.references, draft.references);
  assert.deepEqual(normalized.attachments, draft.attachments);
  assert.equal(draft.to.length, 1);
});

test("reply uses Reply-To and threads the message while reply-all removes the current account and duplicates", () => {
  const reply = replyMessage(detail, "me@example.com", false);
  assert.deepEqual(reply.to, ["reply@example.com"]);
  assert.deepEqual(reply.cc, []);
  assert.equal(reply.subject, "Re: Project notes");
  assert.equal(reply.inReplyTo, "original@example.com");
  assert.deepEqual(reply.references, [
    "thread@example.com",
    "original@example.com",
  ]);
  assert.match(reply.body, /> First line\n> Second line/);
  const all = replyMessage(detail, "me@example.com", true);
  assert.deepEqual(all.to, ["reply@example.com", "teammate@example.com"]);
  assert.deepEqual(all.cc, ["copy@example.com"]);
  assert.deepEqual(all.bcc, []);
  assert.deepEqual(all.attachments, []);
});

test("reply to a sent message addresses original recipients and preserves one reply prefix", () => {
  const sent = {
    ...detail,
    replyTo: [],
    summary: {
      ...detail.summary,
      from: "me@example.com",
      subject: "Re: Existing",
      to: ["recipient@example.com"],
    },
  };
  const reply = replyMessage(sent, "me@example.com", false);
  assert.deepEqual(reply.to, ["recipient@example.com"]);
  assert.equal(reply.subject, "Re: Existing");
});

test("real backend bare message IDs retain reply threading and deduplicate mixed bracket forms", () => {
  const reply = replyMessage(
    {
      ...detail,
      messageId: "one@example.com",
      references: [
        "root@example.com",
        "<root@example.com>",
        "previous@example.com",
        "<one@example.com>",
      ],
    },
    "me@example.com",
    false,
  );
  assert.equal(reply.inReplyTo, "one@example.com");
  assert.deepEqual(reply.references, [
    "root@example.com",
    "previous@example.com",
    "one@example.com",
  ]);
  const malformed = replyMessage(
    {
      ...detail,
      messageId: "<broken@example.com",
      references: [
        "bare@example.com",
        "<valid@example.com>",
        "broken@example.com>",
        "bad\r\n@example.com",
      ],
    },
    "me@example.com",
    false,
  );
  assert.equal(malformed.inReplyTo, null);
  assert.deepEqual(malformed.references, [
    "bare@example.com",
    "valid@example.com",
  ]);
});

test("attachment accounting handles base64 padding and enforces the combined decoded size", () => {
  assert.equal(base64Size("YQ=="), 1);
  assert.equal(base64Size("YWI="), 2);
  assert.equal(base64Size("YWJj"), 3);
  const tooLarge = "A".repeat(Math.ceil((MAX_ATTACHMENT_BYTES + 3) / 3) * 4);
  assert.throws(
    () =>
      prepareSend({
        ...emptyMessage(),
        to: ["a@example.com"],
        attachments: [
          {
            name: "large.bin",
            mimeType: "application/octet-stream",
            dataBase64: tooLarge,
          },
        ],
      }),
    /10 MB/,
  );
});

test("new selections and folder/account invalidation reject responses from previous requests", () => {
  const guard = createRequestGuard();
  const first = guard.next();
  const next = guard.next();
  assert.equal(guard.valid(first), false);
  assert.equal(guard.valid(next), true);
  guard.invalidate();
  assert.equal(guard.valid(next), false);
});

test("older page merge deduplicates messages and uses refreshed flags", () => {
  const first = { ...detail.summary, uid: 10 };
  const refreshed = { ...first, unread: false };
  const older = { ...detail.summary, uid: 9 };
  const merged = mergeMessagePages([first], [refreshed, older]);
  assert.deepEqual(
    merged.map((message) => message.uid),
    [10, 9],
  );
  assert.equal(merged[0].unread, false);
});

test("UIDVALIDITY changes discard old messages even for empty pages and matching numeric UIDs", () => {
  const old = { ...detail.summary, uid: 10, uidValidity: 4 };
  const next = { ...old, uidValidity: 5 };
  assert.deepEqual(mergeMessagePages([old], [next], 5), [next]);
  assert.deepEqual(mergeMessagePages([old], [], 5), []);
  assert.deepEqual(mergeMessagePages([old], [], 4), [old]);
});

test("a refreshed empty page, deleted selected message or changed UID generation clears the previous reader", () => {
  const selected = detail.summary;
  assert.equal(
    selectionRemainsInPage(selected, {
      uidValidity: 4,
      messages: [selected],
      hasMore: false,
    }),
    true,
  );
  assert.equal(
    selectionRemainsInPage(selected, {
      uidValidity: 4,
      messages: [],
      hasMore: false,
    }),
    false,
  );
  assert.equal(
    selectionRemainsInPage(selected, {
      uidValidity: 4,
      messages: [{ ...selected, uid: 18 }],
      hasMore: false,
    }),
    false,
  );
  assert.equal(
    selectionRemainsInPage(selected, {
      uidValidity: 5,
      messages: [{ ...selected, uidValidity: 5 }],
      hasMore: false,
    }),
    false,
  );
});

test("reply to oversized or malformed mail has bounded quoted content and safe thread headers", () => {
  const reply = replyMessage(
    {
      ...detail,
      messageId: "bad\r\nheader",
      references: [
        ...Array.from({ length: 150 }, (_, index) => `<${index}@example.com>`),
        "invalid-id",
        "<bad id@example.com>",
      ],
      textBody: "\n".repeat(1_100_000) + "\u0000",
    },
    "me@example.com",
    false,
  );
  assert.ok(new TextEncoder().encode(reply.body).length < 1024 * 1024);
  assert.match(reply.body, /已截断引用/);
  assert.equal(reply.inReplyTo, null);
  assert.equal(reply.references.length, 100);
  assert.ok(reply.references.every((id) => /^\d+@example.com$/.test(id)));
  assert.equal(reply.body.includes("\u0000"), false);
});

test("draft writes remain ordered, sending can wait for the normalized final draft, and stopping prevents resurrection", async () => {
  const writes = [];
  let finishFirst;
  const firstPending = new Promise((resolve) => {
    finishFirst = resolve;
  });
  const writer = createDraftWriter(async (value) => {
    if (value === "old") await firstPending;
    writes.push(value);
  });
  const old = writer.write("old");
  const normalized = writer.write("normalized for sending");
  await Promise.resolve();
  assert.deepEqual(writes, []);
  finishFirst();
  await Promise.all([old, normalized]);
  await writer.settled();
  assert.deepEqual(writes, ["old", "normalized for sending"]);
  writer.stop();
  await assert.rejects(writer.write("late autosave"), /编辑已结束/);
  assert.deepEqual(writes, ["old", "normalized for sending"]);
});

test("a failed autosave does not block a later explicit save retry", async () => {
  let calls = 0;
  const writes = [];
  const writer = createDraftWriter(async (value) => {
    calls++;
    if (calls === 1) throw new Error("disk unavailable");
    writes.push(value);
  });
  await assert.rejects(writer.write("first"), /disk unavailable/);
  await writer.write("retry");
  assert.deepEqual(writes, ["retry"]);
});
