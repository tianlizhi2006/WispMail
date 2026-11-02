import { createSignal, onMount } from "solid-js";
import { getAppVersion } from "./lib/api";
import { FolderPane } from "./features/folders/FolderPane";
import { MessageList } from "./features/messages/MessageList";
import { MessageView } from "./features/reader/MessageView";
import "./App.css";

/**
 * 应用外壳：经典三栏邮件客户端布局。
 * 左栏文件夹、中栏邮件列表、右栏阅读区。此处仅搭好骨架，各区内容待实现。
 */
function App() {
  const [version, setVersion] = createSignal("");

  onMount(async () => {
    setVersion(await getAppVersion());
  });

  return (
    <div class="app">
      <aside class="pane pane--folders">
        <FolderPane />
      </aside>
      <section class="pane pane--list">
        <MessageList />
      </section>
      <section class="pane pane--reader">
        <MessageView />
      </section>
      <footer class="statusbar">{version()}</footer>
    </div>
  );
}

export default App;
