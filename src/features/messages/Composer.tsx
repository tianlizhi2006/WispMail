import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import {
  clearDraft,
  errorMessage,
  saveDraft,
  sendMessage,
} from "../../lib/api";
import type { Account, ComposeMessage, SendResult } from "../../lib/types";
import { Icon } from "../../ui/Icon";
import {
  base64Size,
  createDraftWriter,
  formatSize,
  MAX_ATTACHMENT_BYTES,
  prepareSend,
} from "./mailHelpers";

export function Composer(props: {
  account: Account;
  initial: ComposeMessage;
  saveInitially: boolean;
  onClose: () => void;
  onSent: (result: SendResult) => void;
}) {
  const [draft, setDraft] = createSignal<ComposeMessage>(props.initial);
  const [busy, setBusy] = createSignal<
    "send" | "close" | "discard" | "attachment" | null
  >(null);
  const [saveStatus, setSaveStatus] = createSignal("草稿自动保存到本机");
  const [error, setError] = createSignal("");
  const [discarding, setDiscarding] = createSignal(false);
  let dialog!: HTMLDialogElement;
  let fileInput!: HTMLInputElement;
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let revision = 0;
  let savedRevision = 0;
  const writer = createDraftWriter<ComposeMessage>((value) =>
    saveDraft(props.account.id, value),
  );

  function cancelScheduledSave() {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  }
  async function persist(value: ComposeMessage, version: number) {
    setSaveStatus("正在保存草稿…");
    await writer.write(value);
    if (alive && version === revision) {
      savedRevision = version;
      setSaveStatus("草稿已保存到本机");
    }
  }
  function scheduleSave(value: ComposeMessage) {
    cancelScheduledSave();
    const version = ++revision;
    setSaveStatus("等待保存草稿…");
    timer = setTimeout(() => {
      timer = undefined;
      void persist(value, version).catch((error) => {
        if (alive && version === revision) {
          setSaveStatus("草稿尚未保存");
          setError(`草稿保存失败：${errorMessage(error)}。请保持此窗口打开。`);
        }
      });
    }, 700);
  }
  function update(patch: Partial<ComposeMessage>) {
    const next = { ...draft(), ...patch };
    setDraft(next);
    setError("");
    scheduleSave(next);
  }
  onMount(() => {
    dialog.showModal();
    if (props.saveInitially) scheduleSave(draft());
  });
  onCleanup(() => {
    alive = false;
    cancelScheduledSave();
    writer.stop();
  });

  async function close() {
    if (busy()) return;
    setBusy("close");
    cancelScheduledSave();
    setError("");
    try {
      if (revision !== savedRevision) await persist(draft(), revision);
      else await writer.settled();
      dialog.close();
      props.onClose();
    } catch (error) {
      setError(
        `草稿未能保存：${errorMessage(error)}。内容仍在此窗口，请重试。`,
      );
    } finally {
      if (alive) setBusy(null);
    }
  }

  async function discard() {
    if (busy()) return;
    setBusy("discard");
    cancelScheduledSave();
    setError("");
    try {
      await writer.settled().catch(() => {});
      await clearDraft(props.account.id);
      writer.stop();
      dialog.close();
      props.onClose();
    } catch (error) {
      setError(errorMessage(error));
    } finally {
      if (alive) setBusy(null);
    }
  }

  async function send() {
    if (busy()) return;
    let message: ComposeMessage;
    try {
      message = prepareSend(draft());
    } catch (error) {
      setError(errorMessage(error));
      return;
    }
    setBusy("send");
    setError("");
    cancelScheduledSave();
    let deliveryStarted = false;
    try {
      // 最后的规范化草稿与发送对象一致，且旧保存全部完成，避免发送后草稿复活。
      await persist(message, revision);
      deliveryStarted = true;
      const result = await sendMessage(props.account.id, message);
      writer.stop();
      dialog.close();
      props.onSent(result);
    } catch (error) {
      setError(
        deliveryStarted
          ? `发送未能确认：${errorMessage(error)}。草稿已保留；请先检查邮箱的“已发送”，确认未发送后再重试，以免重复发送。`
          : `本次尚未发送，草稿保存失败：${errorMessage(error)}。请重试。`,
      );
    } finally {
      if (alive) setBusy(null);
    }
  }

  async function addFiles(files: FileList | null) {
    if (!files?.length || busy()) return;
    setBusy("attachment");
    setError("");
    try {
      const selected = Array.from(files);
      const existingSize = draft().attachments.reduce(
        (sum, file) => sum + base64Size(file.dataBase64),
        0,
      );
      if (
        existingSize + selected.reduce((sum, file) => sum + file.size, 0) >
        MAX_ATTACHMENT_BYTES
      )
        throw new Error("附件总大小不能超过 10 MB。");
      const attachments = [...draft().attachments];
      for (const file of selected) {
        const bytes = new Uint8Array(await file.arrayBuffer());
        let binary = "";
        for (let offset = 0; offset < bytes.length; offset += 32768)
          binary += String.fromCharCode(
            ...bytes.subarray(offset, offset + 32768),
          );
        attachments.push({
          name: file.name,
          mimeType: file.type || "application/octet-stream",
          dataBase64: btoa(binary),
        });
      }
      if (alive) update({ attachments });
    } catch (error) {
      if (alive) setError(errorMessage(error));
    } finally {
      fileInput.value = "";
      if (alive) setBusy(null);
    }
  }

  return (
    <dialog
      ref={dialog}
      class="account-dialog composer-dialog"
      aria-labelledby="composer-title"
      onCancel={(event) => {
        event.preventDefault();
        void close();
      }}
    >
      <header class="dialog-header">
        <div>
          <span class="eyebrow">{props.account.email}</span>
          <h2 id="composer-title">
            {props.initial.inReplyTo ? "回复邮件" : "写邮件"}
          </h2>
        </div>
        <button
          type="button"
          class="icon-button"
          aria-label="保存草稿并关闭"
          disabled={!!busy()}
          onClick={() => void close()}
        >
          <Icon name="close" />
        </button>
      </header>
      <form onSubmit={(event) => event.preventDefault()}>
        <div class="dialog-body composer-body">
          <fieldset class="form-fields composer-fields" disabled={!!busy()}>
            <label>
              收件人
              <input
                name="to"
                autofocus
                autocomplete="off"
                placeholder="name@example.com，多个地址用逗号分隔"
                value={draft().to.join(", ")}
                onInput={(event) => update({ to: [event.currentTarget.value] })}
              />
            </label>
            <div class="form-two-columns">
              <label>
                抄送
                <input
                  name="cc"
                  autocomplete="off"
                  value={draft().cc.join(", ")}
                  onInput={(event) =>
                    update({ cc: [event.currentTarget.value] })
                  }
                />
              </label>
              <label>
                密送
                <input
                  name="bcc"
                  autocomplete="off"
                  value={draft().bcc.join(", ")}
                  onInput={(event) =>
                    update({ bcc: [event.currentTarget.value] })
                  }
                />
              </label>
            </div>
            <label>
              主题
              <input
                name="subject"
                maxlength="998"
                value={draft().subject}
                placeholder="邮件主题"
                onInput={(event) =>
                  update({ subject: event.currentTarget.value })
                }
              />
            </label>
            <label class="composer-text-label">
              <span class="visually-hidden">邮件正文</span>
              <textarea
                name="body"
                class="composer-text"
                placeholder="写下你想说的话…"
                value={draft().body}
                onInput={(event) => update({ body: event.currentTarget.value })}
              />
            </label>
            <input
              ref={fileInput}
              class="visually-hidden"
              type="file"
              multiple
              tabindex="-1"
              aria-label="选择邮件附件"
              onChange={(event) => void addFiles(event.currentTarget.files)}
            />
            <div class="compose-attachments">
              <For each={draft().attachments}>
                {(attachment, index) => (
                  <div class="compose-file">
                    <span title={attachment.name}>{attachment.name}</span>
                    <small>
                      {formatSize(base64Size(attachment.dataBase64))}
                    </small>
                    <button
                      type="button"
                      class="icon-button"
                      aria-label={`移除附件 ${attachment.name}`}
                      onClick={() =>
                        update({
                          attachments: draft().attachments.filter(
                            (_, item) => item !== index(),
                          ),
                        })
                      }
                    >
                      <Icon name="close" />
                    </button>
                  </div>
                )}
              </For>
            </div>
            <div class="attachment-toolbar">
              <button
                type="button"
                class="button button--secondary"
                onClick={() => fileInput.click()}
              >
                <Icon name="plus" />
                添加附件
              </button>
              <span>附件合计不超过 10 MB</span>
            </div>
          </fieldset>
          <Show when={error()}>
            <p class="error-message" role="alert">
              {error()}
            </p>
          </Show>
          <Show when={discarding()}>
            <div class="draft-discard-confirm">
              <p>舍弃这封尚未发送的邮件及附件？</p>
              <div class="button-group">
                <button
                  type="button"
                  class="button button--secondary"
                  disabled={!!busy()}
                  onClick={() => setDiscarding(false)}
                >
                  继续编辑
                </button>
                <button
                  type="button"
                  class="button button--danger"
                  disabled={!!busy()}
                  onClick={() => void discard()}
                >
                  确认舍弃
                </button>
              </div>
            </div>
          </Show>
        </div>
        <footer class="dialog-footer composer-footer">
          <div class="draft-status" aria-live="polite">
            {busy() === "send"
              ? "正在发送，请勿重复操作…"
              : busy() === "attachment"
                ? "正在读取附件…"
                : saveStatus()}
          </div>
          <div class="button-group">
            <button
              type="button"
              class="button button--quiet-danger"
              disabled={!!busy()}
              onClick={() => setDiscarding(true)}
            >
              舍弃
            </button>
            <button
              type="button"
              class="button button--secondary"
              disabled={!!busy()}
              onClick={() => void close()}
            >
              保存并关闭
            </button>
            <button
              type="button"
              class="button button--primary"
              disabled={!!busy()}
              onClick={() => void send()}
            >
              <Icon name="arrow" />
              {busy() === "send" ? "发送中…" : "发送"}
            </button>
          </div>
        </footer>
      </form>
    </dialog>
  );
}
