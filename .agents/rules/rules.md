---
trigger: always_on
glob: "**/*"
description: "WhatMedia 项目开发、多端架构与构建规范"
---

# WhatMedia 项目开发规范与规则指南

## 1. Android 端开发与编译环境规则 (CRITICAL)

- **环境初始化命令规范**：
  在编译或调试 Android 端（例如执行 `yarn tauri android build`、`yarn tauri android dev` 或调用 `gradlew`）时，**必须先在同一个 PowerShell 命令行中执行环境配置脚本**：
  ```powershell
  & "D:\DevTools\AndroidSdk\env.ps1"; yarn tauri android build --apk
  ```
  该脚本已固化了匹配 Gradle 的 JDK（JDK 17）、`ANDROID_HOME`、`NDK_HOME` 以及 `cmdline-tools` / `platform-tools` 等 PATH 环境变量。
- **Android 原生容器与生命周期规范**：
  - [MainActivity.kt](file:///d:/project/WhatMedia/client/src-tauri/gen/android/app/src/main/java/media/whatmedia/client/MainActivity.kt) **必须继承自 `WryActivity`**，禁止退化为普通 `AppCompatActivity`，否则会导致 Tauri WebView 与 Rust 运行时无法初始化而出现**进入黑屏**。
  - `onCreate` 中必须调用 `super.onCreate(savedInstanceState)` 保证 Tauri 运行时与生命周期挂载。
  - 严禁在非播放态启动时调用 `hide(systemBars)` 抹除状态栏。
- **Android 网络与清单声明**：
  - [AndroidManifest.xml](file:///d:/project/WhatMedia/client/src-tauri/gen/android/app/src/main/AndroidManifest.xml) 中 `<application>` 必须显式声明 `android:usesCleartextTraffic="true"`，保证手机与 TV 在家庭局域网内可正常连接私有 HTTP NAS 服务端。
  - 保留 `screenLayout|smallestScreenSize|screenSize|orientation` 等 `configChanges` 与画中画、Leanback TV 属性。

---

## 2. 前端架构与设计系统规范 (Client Architecture)

- **技术栈约束**：
  - Vue 3 (Composition API, `<script setup lang="ts">`) + Vite + Pinia + Vue Router + TypeScript。
  - **样式规范**：严格使用 **Vanilla CSS**，1:1 遵循极光暗黑流体美学与设计系统（见 `ui.html` 规范），**严禁使用 TailwindCSS**。
- **多端与服务地址自适应 (Multi-Server Support)**：
  - 移动端 App（Android/iOS/TV）运行在本地静态协议下，无法依赖同源相对路径 `/api/v1`。
  - 所有 API 请求均走动态获取基础路径 `getBaseUrl()`，优先读取用户在本地配置的服务端地址（`whatmedia.server_url`）。
  - 海报、剧照、分片与流媒体 URL 渲染必须使用 `resolveMediaUrl(path)`，确保在移动端真机壳中自动补全为包含主机 IP/端口的绝对 URL。
  - 严禁在页面或组件中硬编码 `http://localhost:8080` 或裸写未包裹的 `/api/v1/...` 图片与流媒体链接。

---

## 3. 后端架构与 API 契约规范 (Backend & Core)

- **技术栈**：Rust (Workspace 多包架构)，Axum + Tokio + SQLite/DuckDB + tracing。
- **API 端点与 OpenAPI 契约**：
  - 服务端 API 统一遵循 `/api/v1` 前缀与统一错误体 `{ error: { code, message, trace_id } }`。
  - 前端 `client/src/api/schema.d.ts` 由服务端 OpenAPI 规范通过 `make openapi` → `npm run gen:api` 自动生成，禁止手动在前端伪造不一致的契约类型。

---

## 4. 自动化测试与质量防护 (Quality Assurance)

- 任何前端核心模块修改（API、播放器、卡片、设置、路由等），必须运行 `yarn test` 验证回归，确保 **100% 测试通过**。
- 编写或更新单元测试时，Mock `../api/client` 必须优先使用 `importOriginal` 保持 `resolveMediaUrl`、`getServerUrl` 等基础工具导出完整，避免触发 mock 严格模式报错。

---

## 5. 工作流程与 Git 规范 (Workflow & Constraints)

- **用户确认先行**：在接到重大需求或排查指令时，先做深度技术分析与可行性方案规划，经用户确认后再行动。
- **Git 严格受控**：**严禁擅自执行 `git commit` 或 `git push`**。只有当用户明确发出“提交”、“推送”或对应指令时，方可整理规范的 Conventional Commits 并推送到远程。
