/* WispMail 前端入口。
 *
 * 目录按职责分层：
 * - lib/      与 Rust 后端通信的封装、类型定义、工具函数。
 * - features/ 按功能域组织的业务模块（账户、邮件列表、阅读、撰写等）。
 * - ui/       与业务无关的通用界面组件。
 * - App.tsx   应用外壳：三栏布局（文件夹 / 邮件列表 / 阅读区）。
 */
import { render } from "solid-js/web";
import App from "./App";
import "./styles/global.css";

render(() => <App />, document.getElementById("root") as HTMLElement);
