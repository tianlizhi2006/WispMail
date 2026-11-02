import assert from "node:assert/strict";
import test from "node:test";
import {
  mailBaseUrl,
  resolveMailLinkUrl,
  resolveMailResourceUrl,
  validateMailBridgeMessage,
} from "../src/features/reader/mailResourceUrls.ts";
import { rewriteCssUrls } from "../src/features/reader/mailCss.ts";

const png = "iVBORw0KGgo=";
const resources = [
  {
    contentId: "logo@example.com",
    contentLocation: null,
    mimeType: "image/png",
    dataBase64: png,
    attachmentIndex: 2,
  },
  {
    contentId: "literal%40id",
    contentLocation: null,
    mimeType: "image/png",
    dataBase64: png,
    attachmentIndex: 3,
  },
  {
    contentId: null,
    contentLocation: "images/background.png",
    mimeType: "image/png",
    dataBase64: png,
    attachmentIndex: 4,
  },
];
const context = {
  inlineResources: resources,
  baseUrl: "https://mail.example.com/news/",
  allowRemoteImages: true,
};

test("CID scheme is case insensitive and URI references decode exactly once", () => {
  assert.equal(
    resolveMailResourceUrl("CID:logo%40example.com", context).attachmentIndex,
    2,
  );
  assert.equal(
    resolveMailResourceUrl("cid:literal%2540id", context).attachmentIndex,
    3,
  );
  assert.equal(resolveMailResourceUrl("cid:literal%40id", context), null);
  assert.equal(resolveMailResourceUrl("cid:missing", context), null);
});

test("Content-Location maps relative and absolute references to the same local image", () => {
  const relative = resolveMailResourceUrl("images/background.png", context);
  const absolute = resolveMailResourceUrl(
    "https://mail.example.com/news/images/background.png",
    context,
  );
  assert.equal(relative.attachmentIndex, 4);
  assert.equal(absolute.url, relative.url);
  assert.equal(relative.remote, false);
  assert.equal(
    resolveMailResourceUrl("images/background.png", {
      inlineResources: resources,
    }).attachmentIndex,
    4,
  );
});

test("HTTP and HTTPS images load by default, while disabling remote images preserves CID and data images", () => {
  assert.equal(
    resolveMailResourceUrl("https://images.example.com/photo.png").remote,
    true,
  );
  assert.equal(
    resolveMailResourceUrl("http://images.example.com/photo.png").remote,
    true,
  );
  const hidden = { ...context, allowRemoteImages: false };
  assert.equal(
    resolveMailResourceUrl("https://images.example.com/photo.png", hidden),
    null,
  );
  assert.equal(resolveMailResourceUrl("missing-relative.png", hidden), null);
  assert.equal(
    resolveMailResourceUrl("cid:logo@example.com", hidden).attachmentIndex,
    2,
  );
  assert.equal(
    resolveMailResourceUrl(`data:image/png;base64,${png}`, hidden).remote,
    false,
  );
});

test("unknown relative URLs never inherit a local application address", () => {
  assert.equal(resolveMailResourceUrl("/assets/private.png"), null);
  assert.equal(
    resolveMailResourceUrl("../private.png", {
      baseUrl: "file:///C:/private/",
    }),
    null,
  );
  assert.equal(
    resolveMailResourceUrl("/photo.png", {
      baseUrl: "https://mail.example.com/news/",
    }).url,
    "https://mail.example.com/photo.png",
  );
  assert.equal(mailBaseUrl("tauri://localhost/"), null);
  assert.equal(mailBaseUrl("javascript:alert(1)"), null);
});

test("active and local resource schemes are rejected, and inline images may share one renderer-provided resource URL", () => {
  for (const value of [
    "javascript:alert(1)",
    "file:///C:/secret.png",
    "tauri://localhost/key",
    "data:text/html,<script>alert(1)</script>",
    "https://user:password@example.com/image.png",
  ]) {
    assert.equal(resolveMailResourceUrl(value, context), null, value);
  }
  const result = resolveMailResourceUrl("cid:logo@example.com", {
    ...context,
    inlineResourceUrl: () =>
      "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7#wisp-marker-0",
  });
  assert.equal(result.attachmentIndex, 2);
  assert.match(result.url, /#wisp-marker-0$/);
});

test("normal links resolve against the mail base but local and executable link schemes cannot open", () => {
  assert.equal(
    resolveMailLinkUrl("../account", "https://mail.example.com/news/item.html"),
    "https://mail.example.com/account",
  );
  assert.equal(
    resolveMailLinkUrl("mailto:hello@example.com?subject=Hi"),
    "mailto:hello@example.com?subject=Hi",
  );
  assert.equal(resolveMailLinkUrl("#section"), "#section");
  assert.equal(resolveMailLinkUrl("mailto:?subject=Hello"), "mailto:?subject=Hello");
  assert.equal(resolveMailLinkUrl("mailto:"), null);
  for (const value of [
    "javascript:alert(1)",
    "file:///C:/",
    "data:image/png;base64,YQ==",
    "/relative",
    "https://example.com/\nsecret",
  ])
    assert.equal(resolveMailLinkUrl(value), null);
});

test("CSS URL rewriting preserves layout strings while resolving escaped CID and image-set sources", () => {
  const used = new Set();
  const result = rewriteCssUrls(
    'u\\72l("cid:logo%40example.com") center / cover no-repeat',
    context,
    used,
  );
  assert.match(result, /^url\("data:image\/png;base64,/);
  assert.match(result, /center \/ cover no-repeat$/);
  assert.deepEqual([...used], [2]);
  assert.equal(
    rewriteCssUrls('"literal url(https://example.com/text)"', context),
    '"literal url(https://example.com/text)"',
  );
  assert.match(
    rewriteCssUrls(
      'image-set("images/background.png" 1x, url("https://images.example.com/2x.png") 2x)',
      context,
    ),
    /^image-set\(url\("data:image\/png/,
  );
  const hidden = rewriteCssUrls(
    "url(https://images.example.com/image.png), url(cid:logo@example.com)",
    { ...context, allowRemoteImages: false },
  );
  assert.match(hidden, /^none, url\("data:image\/png/);
  assert.equal(
    rewriteCssUrls('url("j\\61vascript:alert(1)")', context),
    "none",
  );
});

test("message bridge requires the document token, expected shape and an allowed link scheme", () => {
  const base = { channel: "wispmail-email", token: "current-document" };
  assert.deepEqual(
    validateMailBridgeMessage(
      { ...base, type: "resize", height: 512.3 },
      base.token,
    ),
    { type: "resize", height: 513 },
  );
  assert.deepEqual(
    validateMailBridgeMessage(
      { ...base, type: "open-link", url: "https://example.com/" },
      base.token,
    ),
    { type: "open-link", url: "https://example.com/" },
  );
  assert.equal(
    validateMailBridgeMessage(
      { ...base, token: "old-document", type: "resize", height: 400 },
      base.token,
    ),
    null,
  );
  assert.equal(
    validateMailBridgeMessage(
      { ...base, type: "resize", height: Infinity },
      base.token,
    ),
    null,
  );
  assert.equal(
    validateMailBridgeMessage(
      { ...base, type: "resize", height: 100001 },
      base.token,
    ),
    null,
  );
  assert.equal(
    validateMailBridgeMessage(
      { ...base, type: "open-link", url: "javascript:alert(1)" },
      base.token,
    ),
    null,
  );
  assert.equal(
    validateMailBridgeMessage(
      { ...base, type: "send-mail", url: "https://example.com/" },
      base.token,
    ),
    null,
  );
});
