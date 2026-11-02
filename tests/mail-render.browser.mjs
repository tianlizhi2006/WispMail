/** Real Chromium regression suite. No browser is downloaded; no existing browser is controlled. */
import assert from "node:assert/strict";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer as createHttpsServer } from "node:https";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { chromium } from "playwright-core";

const project = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = resolve(project, process.env.MAIL_RENDER_ARTIFACTS || "artifacts/mail-render");
const candidates = [
  process.env.PLAYWRIGHT_EXECUTABLE_PATH,
  process.env["ProgramFiles(x86)"] && join(process.env["ProgramFiles(x86)"], "Microsoft/Edge/Application/msedge.exe"),
  process.env.ProgramFiles && join(process.env.ProgramFiles, "Microsoft/Edge/Application/msedge.exe"),
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "/usr/bin/microsoft-edge", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
].filter(Boolean);
let executablePath;
for (const candidate of candidates) {
  try { await access(candidate); executablePath = candidate; break; } catch { /* Try the next installed browser. */ }
}
if (!executablePath) throw new Error("未找到已安装的 Edge/Chrome。请设置 PLAYWRIGHT_EXECUTABLE_PATH；此测试不会下载浏览器。");

await mkdir(output, { recursive: true });
const server = await createServer({
  root: project,
  configFile: join(project, "vite.config.ts"),
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false, open: false, hmr: false },
});
let browser;
let secureServer;
const results = [];
try {
  await server.listen();
  const address = server.httpServer.address();
  assert(address && typeof address !== "string");
  const origin = `http://127.0.0.1:${address.port}`;
  const fixtureDirectory = join(project, "src-tauri/src/sync/test-fixtures");
  const fixtureImage = await readFile(join(project, "src-tauri/icons/32x32.png"));
  secureServer = createHttpsServer({
    key: await readFile(join(fixtureDirectory, "server-key.pem")),
    cert: await readFile(join(fixtureDirectory, "server.pem")),
  }, (request, response) => {
    if (!request.url?.startsWith("/fixture-https.png")) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { "Content-Type": "image/png", "Cache-Control": "no-store" });
    response.end(fixtureImage);
  });
  await new Promise((resolve, reject) => { secureServer.once("error", reject); secureServer.listen(0, "127.0.0.1", resolve); });
  const secureOrigin = `https://127.0.0.1:${secureServer.address().port}`;
  browser = await chromium.launch({ executablePath, headless: true });

  async function scenario(name, suffix, verify) {
    // Only this isolated test browser accepts the local fixture certificate.
    const context = await browser.newContext({ viewport: { width: 1150, height: 900 }, deviceScaleFactor: 1, ignoreHTTPSErrors: true });
    const requests = [];
    const external = [];
    const errors = [];
    const consoleErrors = [];
    await context.route("**/*", async (route) => {
      const url = route.request().url();
      if (url.startsWith(origin + "/") || url.startsWith(secureOrigin + "/")) {
        requests.push(url);
        await route.continue();
      } else if (/^https?:/i.test(url)) {
        external.push(url);
        await route.abort("blockedbyclient");
      } else await route.continue();
    });
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(String(error)));
    page.on("console", (message) => { if (message.type() === "error") consoleErrors.push(message.text()); });
    try {
      const query = new URLSearchParams(suffix.replace(/^\?/, ""));
      query.set("secureOrigin", secureOrigin);
      await page.goto(`${origin}/tests/mail-render.html?${query}`, { waitUntil: "networkidle" });
      await page.waitForFunction(() => document.getElementById("render-state")?.textContent?.startsWith("ready:"), undefined, { timeout: 10000 });
      const handle = await page.$("#mail-frame");
      const frame = await handle.contentFrame();
      assert(frame, "rendered iframe exists");
      await frame.waitForSelector("#cid-image");
      await frame.waitForFunction(() => ["cid-image", "location-image", "data-image", "svg-data-image"].every((id) => {
        const image = document.getElementById(id);
        return image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0;
      }), undefined, { timeout: 10000 });
      await verify({ page, frame, requests, origin });
      assert.equal(await page.locator("#script-breaches").textContent(), "mail script executions: 0");
      assert.deepEqual(external, [], "mail must not attempt network access outside the local test server");
      assert.deepEqual(requests.filter((url) => /forbidden-|missing@|missing-event|unknown-relative/.test(url)), [], "removed active content and unresolved resources must not reach the app server");
      assert.deepEqual(errors, [], "page should have no uncaught JavaScript errors");
      await page.screenshot({ path: join(output, `${name}.png`), fullPage: true });
      results.push({ name, status: "passed", requests: requests.filter((url) => url.includes("fixture=")) });
      process.stdout.write(`PASS ${name}\n`);
    } catch (error) {
      await page.screenshot({ path: join(output, `${name}-failure.png`), fullPage: true }).catch(() => {});
      await writeFile(join(output, `${name}-failure.html`), await page.content());
      const frames = [];
      for (const frame of page.frames().filter((frame) => frame !== page.mainFrame())) {
        frames.push(await frame.evaluate(() => ({
          url: location.href,
          images: [...document.images].map((image) => ({ id: image.id, src: image.getAttribute("src")?.slice(0, 180), complete: image.complete, width: image.naturalWidth })),
          background: document.getElementById("css-cid-background") ? getComputedStyle(document.getElementById("css-cid-background")).backgroundImage.slice(0, 180) : null,
          scripts: [...document.scripts].map((script) => ({ nonce: script.nonce, length: script.textContent.length })),
        })).catch((error) => ({ error: String(error) })));
      }
      results.push({ name, status: "failed", error: String(error), requests, external, errors, consoleErrors, frames });
      throw error;
    } finally { await context.close(); }
  }

  await scenario("complete-rendering", "", async ({ page, frame, requests }) => {
    await frame.waitForFunction(() => document.getElementById("remote-image")?.naturalWidth > 0);
    await frame.waitForFunction(() => document.getElementById("remote-https-image")?.naturalWidth > 0);
    const snapshot = await frame.evaluate(async () => {
      const byId = (id) => document.getElementById(id);
      const background = getComputedStyle(byId("css-cid-background")).backgroundImage;
      const url = background.match(/^url\(["']?(.*?)["']?\)$/)?.[1];
      let backgroundLoaded = false;
      if (url) {
        const image = new Image(); image.src = url;
        try { await image.decode(); backgroundLoaded = image.naturalWidth > 0; } catch { /* Assertion below describes a broken background. */ }
      }
      return {
        color: getComputedStyle(byId("head-styled")).color,
        fontSize: getComputedStyle(byId("head-styled")).fontSize,
        inlineBackground: getComputedStyle(byId("inline-styled")).backgroundColor,
        legacyBackground: getComputedStyle(byId("legacy-background")).backgroundColor,
        tableCount: document.querySelectorAll("table").length,
        background, backgroundLoaded,
        styleText: [...document.querySelectorAll("style")].map((style) => style.textContent).join("\n"),
        scripts: [...document.querySelectorAll("script")].map((script) => ({ nonce: script.nonce, hasMailPayload: script.textContent.includes("fixture-mail-script-ran") })),
        forbiddenTags: document.querySelectorAll("form,input,button,iframe,object,embed,svg,link,base,meta[http-equiv='refresh']").length,
        eventAttributes: [...document.querySelectorAll("*")].flatMap((element) => [...element.attributes].filter((attribute) => /^on/i.test(attribute.name)).map((attribute) => attribute.name)),
        unsafeLinks: [...document.querySelectorAll("a")].filter((link) => /^(javascript|file):/i.test(link.getAttribute("href") || "")).length,
        unknownCid: byId("unknown-cid")?.getAttribute("src") ?? null,
      };
    });
    assert.equal(snapshot.color, "rgb(23, 91, 76)", "head stylesheet survives");
    assert.equal(snapshot.fontSize, "26px");
    assert.equal(snapshot.inlineBackground, "rgb(242, 246, 239)", "inline CSS survives");
    assert.equal(snapshot.legacyBackground, "rgb(229, 239, 247)", "legacy bgcolor survives");
    assert(snapshot.tableCount >= 2, "newsletter table structure survives");
    assert(/^url\("(?:blob:|data:image\/png;)/.test(snapshot.background), "CID CSS background resolves to a local raster resource");
    assert(snapshot.backgroundLoaded, "CSS CID image decodes successfully");
    assert(snapshot.styleText.includes("@media"), "responsive CSS survives");
    assert(!snapshot.styleText.includes("@import"), "stylesheet imports are removed");
    assert.equal(snapshot.forbiddenTags, 0);
    assert.deepEqual(snapshot.eventAttributes, []);
    assert.equal(snapshot.unsafeLinks, 0);
    assert.equal(snapshot.scripts.length, 1, "only the trusted nonce bridge remains");
    assert(snapshot.scripts[0].nonce && !snapshot.scripts[0].hasMailPayload);
    assert(!snapshot.unknownCid || snapshot.unknownCid.startsWith("data:"), "unknown CID cannot become an app relative URL");
    const used = JSON.parse((await page.locator("#used-attachments").textContent()).replace("inline attachments: ", ""));
    assert.deepEqual(used.sort(), [0, 1, 2], "inline resources are accounted for separately from downloadable attachments");
    assert(requests.some((url) => url.includes("fixture=remote-image")), "HTTP image loads by default");
    assert(requests.some((url) => url.includes("fixture=remote-https")), "HTTPS image loads by default");
    assert(requests.some((url) => url.includes("fixture=remote-background")), "HTTP CSS background loads by default");
    await page.waitForFunction(() => /^bridge height: \d+/.test(document.getElementById("bridge-height")?.textContent || ""));
    await frame.locator("#safe-link").click();
    await page.waitForFunction(() => document.getElementById("bridge-link")?.textContent?.includes("https://example.test/fixture-link"));
    assert(frame.url().startsWith("blob:"), "clicking a link cannot navigate the email frame");
    await page.selectOption("#frame-width", "420");
    await frame.waitForFunction(() => getComputedStyle(document.getElementById("head-styled")).fontSize === "18px");
    await page.screenshot({ path: join(output, "responsive-420.png"), fullPage: true });
    await page.selectOption("#frame-width", "680");
  });

  await scenario("remote-images-disabled", "?remote=off", async ({ frame, requests }) => {
    await frame.waitForTimeout(250);
    assert.deepEqual(requests.filter((url) => url.includes("fixture=remote-")), [], "disabled HTTP images/backgrounds must not make a request");
    const state = await frame.evaluate(() => ({
      cid: document.getElementById("cid-image").naturalWidth,
      location: document.getElementById("location-image").naturalWidth,
      data: document.getElementById("data-image").naturalWidth,
      remoteSrc: document.getElementById("remote-image")?.getAttribute("src") ?? null,
      remoteBackground: getComputedStyle(document.getElementById("remote-background")).backgroundImage,
    }));
    assert(state.cid > 0 && state.location > 0 && state.data > 0, "turning off remote images keeps MIME/data images");
    assert(!state.remoteSrc || state.remoteSrc.startsWith("data:"));
    assert.equal(state.remoteBackground, "none");
  });

  await scenario("relative-without-base", "?base=none", async ({ frame, requests }) => {
    const source = await frame.evaluate(() => document.getElementById("unknown-relative")?.getAttribute("src") ?? null);
    assert(!source || source.startsWith("data:"));
    assert(!requests.some((url) => url.includes("unknown-relative.png")));
  });

  // Load the actual App/MessageView through the development IPC shim as well.
  const appContext = await browser.newContext({ viewport: { width: 1440, height: 1100 }, deviceScaleFactor: 1 });
  const appRequests = [];
  const appExternal = [];
  const appErrors = [];
  const appDocumentNavigations = [];
  await appContext.route("**/*", async (route) => {
    const url = route.request().url();
    if (url.startsWith(origin + "/")) { appRequests.push(url); await route.continue(); }
    else if (/^https?:/i.test(url)) { appExternal.push(url); await route.abort("blockedbyclient"); }
    else await route.continue();
  });
  const appPage = await appContext.newPage();
  appPage.on("pageerror", (error) => appErrors.push(String(error)));
  appPage.on("framenavigated", (frame) => { if (frame.url().startsWith("blob:")) appDocumentNavigations.push(frame.url()); });
  try {
    await appPage.goto(`${origin}/tests/mail-ui.html`, { waitUntil: "networkidle" });
    await appPage.getByRole("button", { name: /图文邮件完整渲染样例/ }).click();
    await appPage.getByRole("button", { name: "标为未读", exact: true }).waitFor();
    const frameElement = await appPage.$("iframe.html-message");
    let appFrame = await frameElement.contentFrame();
    await appFrame.waitForFunction(() => document.getElementById("cid-image")?.naturalWidth > 0);
    await appFrame.waitForFunction(() => document.getElementById("svg-data-image")?.naturalWidth > 0);
    assert.equal(appDocumentNavigations.length, 1, "automatic read-state writeback must not reload the email document");
    assert.equal(await appPage.locator(".reader-attachments .download-attachment").count(), 1, "only the true file attachment stays in the HTML view");
    assert.match(await appPage.locator(".reader-attachments").textContent(), /项目说明\.txt/);
    const initialUrl = await frameElement.getAttribute("src");
    await appPage.getByRole("button", { name: "标为未读", exact: true }).click();
    await appPage.getByRole("button", { name: "标为已读", exact: true }).waitFor();
    assert.equal(await frameElement.getAttribute("src"), initialUrl, "read-state updates must not replace the HTML document");
    await appFrame.locator("#safe-link").click();
    await appPage.waitForFunction(() => document.getElementById("fixture-counts")?.textContent?.includes("open_email_link:1"));
    assert.equal(appPage.url(), `${origin}/tests/mail-ui.html`, "link handling must not navigate the app");
    await appPage.locator(".reader-pane").evaluate((pane) => { pane.scrollTop = 0; });
    await appPage.screenshot({ path: join(output, "actual-message-view.png"), fullPage: true });
    const remoteCount = appRequests.filter((url) => url.includes("fixture=remote-")).length;
    await appPage.getByRole("button", { name: "隐藏网络图片", exact: true }).click();
    await appFrame.waitForURL((url) => url.href.startsWith("blob:") && url.href !== initialUrl);
    await appFrame.waitForFunction(() => document.getElementById("cid-image")?.naturalWidth > 0);
    assert.equal(await appFrame.locator("#remote-image").getAttribute("src"), null);
    const hiddenUrl = await frameElement.getAttribute("src");
    await appPage.getByRole("button", { name: "标为已读", exact: true }).click();
    await appPage.getByRole("button", { name: "标为未读", exact: true }).waitFor();
    assert.equal(await frameElement.getAttribute("src"), hiddenUrl, "read-state updates must keep remote images hidden");
    assert.equal(appRequests.filter((url) => url.includes("fixture=remote-")).length, remoteCount);
    await appPage.getByRole("button", { name: "纯文本", exact: true }).click();
    assert.equal(await appPage.locator("iframe.html-message").count(), 0);
    assert.equal(await appPage.locator(".reader-attachments .download-attachment").count(), 4, "plain view still lets users save MIME images");
    await appPage.getByRole("button", { name: "显示排版", exact: true }).click();
    appFrame = await (await appPage.$("iframe.html-message")).contentFrame();
    await appFrame.waitForFunction(() => document.getElementById("cid-image")?.naturalWidth > 0);
    assert.equal(await appFrame.locator("#remote-image").getAttribute("src"), null);
    assert.equal(appRequests.filter((url) => url.includes("fixture=remote-")).length, remoteCount);
    assert.deepEqual(appExternal, []);
    assert.deepEqual(appErrors, []);
    await appPage.locator(".reader-pane").evaluate((pane) => { pane.scrollTop = 0; });
    await appPage.screenshot({ path: join(output, "actual-message-view-remote-off.png"), fullPage: true });
    results.push({ name: "actual-message-view", status: "passed" });
    process.stdout.write("PASS actual-message-view\n");
  } catch (error) {
    await appPage.screenshot({ path: join(output, "actual-message-view-failure.png"), fullPage: true }).catch(() => {});
    results.push({ name: "actual-message-view", status: "failed", error: String(error), requests: appRequests, external: appExternal, errors: appErrors });
    throw error;
  } finally { await appContext.close(); }
  process.stdout.write(`Screenshots: ${output}\n`);
} finally {
  await writeFile(join(output, "results.json"), JSON.stringify({ browser: executablePath, results }, null, 2));
  await browser?.close();
  if (secureServer) { secureServer.closeAllConnections(); await new Promise((resolve) => secureServer.close(resolve)); }
  await server.close();
}
