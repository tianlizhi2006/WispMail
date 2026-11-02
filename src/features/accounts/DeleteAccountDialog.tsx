import { createSignal, onMount, Show } from "solid-js";
import { deleteAccount, errorMessage } from "../../lib/api";
import type { Account } from "../../lib/types";
import { Icon } from "../../ui/Icon";

export function DeleteAccountDialog(props: {
  account: Account;
  onClose: () => void;
  onDeleted: (id: string) => void;
}) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let dialog!: HTMLDialogElement;
  onMount(() => dialog.showModal());
  function close() {
    if (!busy()) {
      dialog.close();
      props.onClose();
    }
  }
  async function remove() {
    if (busy()) return;
    setBusy(true);
    setError("");
    try {
      await deleteAccount(props.account.id);
      dialog.close();
      props.onDeleted(props.account.id);
    } catch (error) {
      setError(errorMessage(error));
      setBusy(false);
    }
  }
  return (
    <dialog
      ref={dialog}
      class="account-dialog delete-dialog"
      aria-labelledby="delete-title"
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
    >
      <div class="dialog-body">
        <div class="delete-symbol">
          <Icon name="trash" />
        </div>
        <h2 id="delete-title">移除此邮箱账户？</h2>
        <p class="delete-email">{props.account.email}</p>
        <p class="muted">
          将删除本机账户配置、登录凭据、缓存邮件和未发送的本地草稿，邮箱服务器上的邮件不受影响。
        </p>
        <Show when={error()}>
          <p class="error-message" role="alert">
            {error()}
          </p>
        </Show>
      </div>
      <footer class="dialog-footer">
        <div class="button-group">
          <button
            class="button button--secondary"
            type="button"
            autofocus
            disabled={busy()}
            onClick={close}
          >
            取消
          </button>
          <button
            class="button button--danger"
            type="button"
            disabled={busy()}
            onClick={() => void remove()}
          >
            {busy() ? "正在移除…" : "移除账户"}
          </button>
        </div>
      </footer>
    </dialog>
  );
}
