import { renderMailDocument } from "../src/features/reader/safeHtml";
import { completeHtmlFixture, inlineResources } from "./mail-render-fixture";

const frame = document.getElementById("mail-frame") as HTMLIFrameElement;
const allowRemote = document.getElementById("allow-remote") as HTMLInputElement;
const width = document.getElementById("frame-width") as HTMLSelectElement;
const renderState = document.getElementById("render-state")!;
const used = document.getElementById("used-attachments")!;
const height = document.getElementById("bridge-height")!;
const link = document.getElementById("bridge-link")!;
const breaches = document.getElementById("script-breaches")!;
let url: string | undefined;
let disposeResources: (() => void) | undefined;
let token = "";
let renderNumber = 0;
let scriptBreaches = 0;
allowRemote.checked = new URLSearchParams(location.search).get("remote") !== "off";

window.addEventListener("message", (event) => {
  if (event.source !== frame.contentWindow) return;
  if (event.data?.channel === "fixture-mail-script-ran") {
    scriptBreaches++;
    breaches.textContent = `mail script executions: ${scriptBreaches}`;
    return;
  }
  if (event.data?.channel !== "wispmail-email" || event.data?.token !== token) return;
  if (event.data.type === "resize" && typeof event.data.height === "number") {
    height.textContent = `bridge height: ${event.data.height}`;
    frame.style.height = `${Math.min(5000, Math.max(300, event.data.height))}px`;
  }
  if (event.data.type === "open-link" && typeof event.data.url === "string") {
    link.textContent = `bridge link: ${event.data.url}`;
  }
});

function renderFixture() {
  try {
    token = `fixture-token-${crypto.randomUUID()}`;
    const noBase = new URLSearchParams(location.search).get("base") === "none";
    const secureOrigin = new URLSearchParams(location.search).get("secureOrigin") ?? undefined;
    const fixtureHtml = completeHtmlFixture(location.origin, secureOrigin);
    const html = noBase ? fixtureHtml.replace(/<base\b[^>]*>/gi, "") + '<img id="unknown-relative" src="unknown-relative.png">' : fixtureHtml;
    const result = renderMailDocument(html, {
      inlineResources,
      htmlBaseUrl: noBase ? null : `${location.origin}/tests/mail-assets/newsletter.html`,
      ...(allowRemote.checked ? {} : { allowRemoteImages: false }),
      bridgeToken: token,
      scriptNonce: crypto.randomUUID().replace(/-/g, ""),
    });
    const previous = url;
    const previousResources = disposeResources;
    disposeResources = result.dispose;
    url = URL.createObjectURL(new Blob([result.html], { type: "text/html" }));
    frame.src = url;
    if (previous) URL.revokeObjectURL(previous);
    previousResources?.();
    used.textContent = `inline attachments: ${JSON.stringify(result.usedAttachmentIndexes)}`;
    renderState.textContent = `ready:${++renderNumber} · remote:${allowRemote.checked ? "on" : "off"}`;
    document.getElementById("renderer-error")!.textContent = "";
  } catch (error) {
    document.getElementById("renderer-error")!.textContent = String(error);
    renderState.textContent = "failed";
  }
}
allowRemote.addEventListener("change", renderFixture);
width.addEventListener("change", () => { frame.style.width = `${width.value}px`; });
document.getElementById("render-again")!.addEventListener("click", renderFixture);
window.addEventListener("pagehide", () => { if (url) URL.revokeObjectURL(url); disposeResources?.(); });
renderFixture();
