import assert from "node:assert/strict";
import test from "node:test";
import {
  base64Size,
  checkedMessages,
  composeKind,
  createDraftWriter,
  createLruCache,
  createRequestGuard,
  emptyMessage,
  forwardMessage,
  MAX_ATTACHMENT_BYTES,
  mergeMessagePages,
  messageKey,
  moveTargets,
  parseAddresses,
  prepareSend,
  removeFromList,
  replyMessage,
  selectionRemainsInPage,
  splitAddresses,
  toggleChecked,
} from "../src/features/messages/mailHelpers.ts";

const row = (uid, uidValidity = 9) => ({
  uid,
  uidValidity,
  subject: `m${uid}`,
  from: "a@example.com",
  to: [],
  date: "",
  unread: false,
  flagged: false,
  size: 1,
  snippet: "",
  hasAttachments: false,
});

test("removing the open message selects the next older one, then the newer one at the end", () => {
  const list = [row(5), row(4), row(3), row(2)];
  let result = removeFromList(list, 9, [4], list[1]);
  assert.deepEqual(
    result.messages.map((m) => m.uid),
    [5, 3, 2],
  );
  assert.equal(result.next?.uid, 3);
  result = removeFromList(list, 9, [2], list[3]);
  assert.equal(result.next?.uid, 3, "last row falls back to the previous one");
  result = removeFromList([row(1)], 9, [1], row(1));
  assert.deepEqual(result.messages, []);
  assert.equal(result.next, undefined);
});

test("removal ignores other UID generations and keeps an unaffected selection", () => {
  const list = [row(5), row(4, 8), row(3)];
  const result = removeFromList(list, 9, [4, 3], list[0]);
  assert.deepEqual(
    result.messages.map((m) => `${m.uidValidity}:${m.uid}`),
    ["9:5", "8:4"],
  );
  assert.equal(result.next, undefined, "selection was not removed");
});

test("checkbox toggles one message and Shift selects the whole range from the anchor", () => {
  const list = [row(5), row(4), row(3), row(2)];
  let checked = toggleChecked(list, new Set(), list[0]);
  assert.deepEqual([...checked], ["9:5"]);
  checked = toggleChecked(list, checked, list[2], messageKey(list[0]));
  assert.deepEqual([...checked].sort(), ["9:3", "9:4", "9:5"]);
  checked = toggleChecked(list, checked, list[1]);
  assert.deepEqual(
    [...checked].sort(),
    ["9:3", "9:5"],
    "plain toggle unchecks",
  );
  // 锚点已不在列表中：退化为普通切换。
  checked = toggleChecked(list, new Set(), list[3], "9:99");
  assert.deepEqual([...checked], ["9:2"]);
});

test("checked messages follow the current list and ignore other UID generations", () => {
  const list = [row(5), row(4), row(3)];
  const checked = new Set(["9:5", "9:3", "9:1", "8:4"]);
  assert.deepEqual(
    checkedMessages(list, checked).map((m) => m.uid),
    [5, 3],
  );
  assert.deepEqual(checkedMessages(list, new Set()), []);
});

test("forward keeps original headers, body and attachments without reply threading", () => {
  const attachments = [
    { name: "a.pdf", mimeType: "application/pdf", dataBase64: "AAECAw==" },
  ];
  const forwarded = forwardMessage(detail, attachments);
  assert.equal(forwarded.subject, "Fwd: Project notes");
  assert.deepEqual(forwarded.to, []);
  assert.equal(forwarded.inReplyTo, null);
  assert.deepEqual(forwarded.references, []);
  assert.deepEqual(forwarded.attachments, attachments);
  assert.match(forwarded.body, /---------- 转发的邮件 ----------/);
  assert.match(forwarded.body, /主题：Project notes/);
  assert.match(forwarded.body, /抄送：copy@example.com/);
  assert.match(forwarded.body, /First line\nSecond line/);
  assert.equal(composeKind(forwarded), "forward");
  assert.equal(
    composeKind(replyMessage(detail, "me@example.com", false)),
    "reply",
  );
  assert.equal(composeKind(emptyMessage()), "new");
  const again = forwardMessage({
    ...detail,
    summary: { ...detail.summary, subject: "转发：周报\r\nBcc: x@y.z" },
  });
  assert.equal(
    again.subject,
    "转发：周报  Bcc: x@y.z",
    "no double prefix or header injection",
  );
});

test("move targets exclude the current and non-selectable folders", () => {
  const folders = [
    { name: "INBOX", selectable: true },
    { name: "Parent", selectable: false },
    { name: "Archive", selectable: true },
  ];
  assert.deepEqual(
    moveTargets(folders, "INBOX").map((f) => f.name),
    ["Archive"],
  );
});

test("LRU cache evicts the least recently read entry", () => {
  const cache = createLruCache(2);
  cache.set("a", 1);
  cache.set("b", 2);
  cache.get("a");
  cache.set("c", 3);
  assert.equal(cache.get("b"), undefined);
  assert.equal(cache.get("a"), 1);
  assert.equal(cache.get("c"), 3);
  cache.delete("a");
  assert.equal(cache.get("a"), undefined);
});

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
