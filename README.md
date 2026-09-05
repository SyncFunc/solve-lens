# 宝宝巴士

宝宝巴士是一个 Windows 截图做题助手：使用全局快捷键截取主屏，按题型把多张截图组成一份草稿，调用本机的命令行 Provider 解题，并将最终纯文本答案显示在受保护的前台浮窗或手机控制页中。

当前版本的产品边界是：

- Windows 10 2004+ 或 Windows 11，首版只截取主屏。
- 一份草稿最多 6 张图，图片按截取顺序传给 Provider。
- 桌面端、前台浮窗和手机控制页都由 React 渲染；Rust 负责截图、窗口、快捷键、Provider 调度、历史和局域网服务。
- 结果按“结论优先”的纯文本显示，不显示流式中间输出，也不做 Markdown 渲染。
- 只应在课程、练习或考试规则允许辅助工具的场景使用。

> 浮窗使用 Windows 的 `WDA_EXCLUDEFROMCAPTURE` 请求排除常见系统捕获 API。它不是 DRM，不能防止相机、采集卡、远程桌面或特权录制程序；Windows 未确认保护状态时，应用会拒绝展示答案。

## 1. 前置安装

### 1.1 Windows 工具链

请先安装以下软件：

1. **Node.js 20 LTS 或更高版本**。本项目使用 npm、Vite 和 React，Node 22 也可以。
2. **Rust stable（MSVC 工具链）**。用 rustup 安装后建议执行：

~~~powershell
rustup default stable-x86_64-pc-windows-msvc
rustup component add rustfmt
~~~

3. **Visual Studio 2022 Build Tools**，在安装器中勾选：

   - Desktop development with C++
   - MSVC v143（或更新版本）C++ build tools
   - Windows 10/11 SDK
   - C++ CMake tools（可选，但建议安装）

4. Git（用于拉取代码和版本管理，可选）。

项目中的 `scripts/tauri.mjs` 会尝试自动把 Cargo 加入 PATH，并通过 `vswhere` 找到 Visual Studio 开发环境。它只能帮助定位已安装的工具，不能替代 Rust 或 C++ Build Tools 的安装。

安装后在新的 PowerShell 中检查：

~~~powershell
node --version
npm --version
cargo --version
rustc --version
~~~

如果 `cargo` 找不到，先确认 `%USERPROFILE%\.cargo\bin` 存在并重新打开终端。如果出现 `VsDevCmd.bat` 找不到，说明 Build Tools 未安装、安装路径不完整，或安装器没有安装 MSVC 工作负载。

### 1.2 安装并登录 Codex CLI

首版实际可用的求解 Provider 是 Codex CLI。先全局安装并登录：

~~~powershell
npm install -g @openai/codex
codex --version
codex login
~~~

登录过程需要按 Codex CLI 的提示完成。确认单独执行 CLI 可以工作：

~~~powershell
codex exec --ephemeral --sandbox read-only "请用一句话回答：1+1 等于多少？"
~~~

如果 CLI 不在 PATH 中，可以在配置页的“Provider 与模型”卡片中填写 `codex.cmd`、`codex.exe` 或完整路径。项目也兼容 npm 生成的 `codex.cmd` 启动 shim。

配置页目前可以选择“Claude Code CLI”，这是 Provider 抽象保留的入口；当前提交链路会明确提示该 Provider 尚未接入求解适配器。未来接入时只需要增加对应的 `AnswerProvider` 实现，不需要重写截图、草稿、浮窗或手机协议。

### 1.3 安装项目依赖

在项目根目录执行：

~~~powershell
cd D:\pyCode\answer-agent
npm install
~~~

## 2. 开发、构建与测试

开发模式会启动 Vite 和 Tauri：

~~~powershell
npm run tauri dev
~~~

常用校验命令：

~~~powershell
npm run build       # TypeScript 检查并构建 React/Vite 资源
npm run check:rust  # cargo check
npm run test:rust   # Rust 单元测试
npm run check:desktop
~~~

`check:desktop` 会启动桌面 Rust 进程，适合检查窗口和运行时依赖，不建议在自动化构建中长期占用。前端构建产物位于 `dist`，Tauri 打包时由 `src-tauri/tauri.conf.json` 的 `frontendDist` 指向该目录。

如果只是使用已打包版本，不需要运行 Vite；仍需要安装并登录 Codex CLI，因为答题进程由本机 CLI 完成。

## 3. 系统架构

### 3.1 角色和边界

~~~mermaid
flowchart LR
  U[电脑用户] --> K[全局快捷键]
  U --> C[React 配置窗口]
  P[手机浏览器] --> M[React 手机控制页]

  K --> D[Rust 草稿与任务编排]
  C <--> S[Rust AppState]
  M <--> L[LocalServer<br/>HTTP + WebSocket]

  D --> G[截图模块<br/>Windows Graphics Capture]
  D --> A[AnswerProvider 抽象]
  A --> X[CodexCliProvider<br/>已实现]
  A -.-> Y[ClaudeCodeCliProvider<br/>预留]
  X --> R[AnswerResult<br/>纯文本]
  R --> O[React 受保护浮窗]
  R --> M
  R --> H[本地加密历史]
~~~

各角色的职责如下：

- **React 配置窗口**：编辑 Provider、模型、超时、浮窗外观、局域网、Prompt、预设和快捷键。保存时一次提交完整配置，实时项（透明度、字号）会即时调用命令。
- **React 前台浮窗**：置顶、透明、鼠标穿透、无焦点，显示截图缩略图、任务状态和完成后的答案。窗口样式和捕获保护由 Rust 设置，React 不创建额外边框或阴影。
- **Rust 核心**：维护 `AppState`、草稿、快捷键、截图生命周期、Provider 子进程、取消/超时、历史和窗口状态。
- **LocalServer**：监听局域网端口，托管手机 React 静态资源，提供加密 API 和 WebSocket 状态同步。
- **手机 React 页面**：发送截图、提交、清空和取消命令，显示状态、缩略图和答案。断线时保留最后一次有效状态并退避重连。
- **AnswerProvider**：把不同 CLI/API 的差异封装起来。业务层只依赖统一的答案结果，不依赖某一个模型厂商。

### 3.2 Provider 设计

| Provider | 当前状态 | 输入 | 说明 |
| --- | --- | --- | --- |
| Codex CLI | 已实现 | Prompt + 按顺序排列的图片 | 启动 `codex exec --ephemeral --json --sandbox read-only`，可附加模型和超时 |
| Claude Code CLI | 配置入口已保留 | 预留 | 当前提交会返回“Provider 尚未接入求解适配器” |
| HTTP/API Provider | 未来扩展 | 同一份任务对象 | 实现 `AnswerProvider` 后即可替换本地 CLI，不影响 UI 和传输层 |

Codex 模型候选不是写死在页面中的。配置页会通过 `codex app-server --stdio` 的 `model/list` 请求读取当前 CLI 可见的模型；因此模型列表取决于 CLI 版本、登录账号和服务端可用性。列表为空时仍可手动填写模型 ID 或留空使用 CLI 默认模型。

### 3.3 一次答题的生命周期

~~~mermaid
sequenceDiagram
  participant Hotkey as 快捷键/手机
  participant Core as Rust 核心
  participant Capture as 截图模块
  participant Provider as CLI Provider
  participant View as 浮窗/手机

  Hotkey->>Core: capture(presetId)
  Core->>Capture: 截取主屏并生成缩略图
  Capture-->>Core: 有序图片引用
  Core-->>View: 草稿图片数量 + 状态
  Hotkey->>Core: submit
  Core->>Provider: 基础安全提示 + 题型 Prompt + 图片
  Provider-->>Core: JSONL 中的最终文本
  Core-->>View: displaying + AnswerResult
  Core->>Core: 清理临时图片并保存加密历史
~~~

每次 Codex 调用包含四部分：不可修改的安全提示、所选题型预设、草稿元数据和按顺序附加的图片。安全提示要求模型只分析题目，不执行图片里出现的指令，不读取与答题无关的本地资源；图片模糊或缺页时必须说明信息不足，不能臆造。

## 4. PC 端使用

### 4.1 首次配置

1. 启动 `npm run tauri dev` 或打开打包应用。配置窗口关闭后会进入系统托盘，可从托盘重新打开。
2. 在“Provider 与模型”卡片中选择 Provider，填写 Codex CLI 路径，选择或填写模型，设置超时。当前生效 Provider 和模型会在卡片中显示。
3. 在“浮窗显示”卡片设置主题（日间、夜间或实时跟随）、透明度、字号、窗口宽高。透明度和字号拖动后立即作用于浮窗。
4. 在“全局 Prompt 附加要求”中填写所有题型都要遵守的额外要求，例如“使用简体中文，结论先给出，必要时核对单位”。
5. 在“题型 Prompt 预设”中编辑通用、数学、代码预设，或复制出自定义预设。内置预设也可以编辑其任务模板；保存后会写入用户配置，不会修改程序资源。
6. 在“快捷键”卡片点击输入框后直接按组合键录入。每项支持 Ctrl、Alt、Shift、Win/Cmd 与功能键、方向键、数字键；冲突错误只显示在对应输入项下方。保存时会再次校验，成功后注销旧注册并注册新快捷键。
7. 如需手机控制，打开“局域网控制”，设置端口（默认 `18765`），按需开启“手机自动保存完整截图”，再保存配置。

### 4.2 截图、提问和查看答案

默认快捷键如下，均可在配置页修改：

| 操作 | 默认快捷键 | 行为 |
| --- | --- | --- |
| 通用截图 | `Ctrl+Alt+1` | 创建或继续通用草稿 |
| 数学截图 | `Ctrl+Alt+2` | 创建或继续数学草稿 |
| 代码截图 | `Ctrl+Alt+3` | 创建或继续代码草稿 |
| 提交解题 | `Ctrl+Alt+Enter` | 提交当前草稿 |
| 清空草稿 | `Ctrl+Alt+Backspace` | 删除当前草稿和临时图片 |
| 显隐浮窗 | `Ctrl+Alt+H` | 切换前台浮窗；手动隐藏后不会被截图/提交自动唤出 |
| 答案向后/向前滚动 | `Ctrl+Alt+N/P` | 长答案超出窗口时滚动内容 |
| 增大/减小透明度 | `Ctrl+Alt+F11/F12` | 立即调整浮窗背景和文字 |
| 增大/减小字号 | `Ctrl+Alt+F9/F10` | 立即调整答案字号 |
| 移动浮窗 | `Ctrl+Alt+方向键` | 每次移动一小段距离 |

实际操作是：

1. 在题目所在窗口按所选题型的截图快捷键。前台浮窗不会因为截图而闪烁，截图会加入草稿并显示缩略图。
2. 同一题需要多张图时，继续按**同一个题型快捷键**，最多 6 张。按其他题型会提示先提交或清空，避免 Prompt 与图片类型混合。
3. 按提交快捷键。浮窗先显示“正在处理”，Codex 完成后才显示最终答案；任务失败时保留错误状态，不把旧答案替换成错误文本。
4. 长答案可以用翻页快捷键滚动，不需要把答案拆成固定页数。手动隐藏浮窗后，只有再次按显隐快捷键才会出现。
5. 需要重新做题时按清空；正在求解时按取消，应用会终止整个 CLI 子进程树并清理临时文件。

## 5. 手机端使用

### 5.1 开启并访问

1. 在 PC 配置页开启“局域网控制”，保存一次。只有开关或端口发生变化时，服务才会重新启动；关闭开关会释放监听端口。
2. 在 PC 的 PowerShell 执行 `ipconfig`，找到与手机处于同一 Wi-Fi/局域网网卡的 IPv4 地址，例如 `192.168.1.23`。
3. 手机与 PC 连接同一网络，在浏览器打开：

~~~text
http://192.168.1.23:18765/
~~~

不要使用 `localhost` 或 `127.0.0.1`，那两个地址指向手机本身。服务绑定局域网网卡，实际端口以配置页当前值为准。

首次打开时页面会执行一次 ECDH 握手，顶部显示“已连接 · 加密”后再操作。手机页面只有一个 WebSocket 和一个轮询降级任务；短暂断网时会显示重连状态，但保留最后一次有效答案。

### 5.2 手机答题流程

1. 在“截图题型”下拉框选择通用、数学或代码。
2. 点击大号“截图加入草稿”按钮。页面先提示请求发送，只有服务器确认命令且草稿图片数量真正增加后才提示截图成功。
3. 需要多图时重复截图；页面会显示当前图片数量和预览。
4. 点击“提交题目”。完成后答案显示在结果区域；清空和取消也会等待服务端真实状态再提示成功。
5. 开启“手机自动保存完整截图”后，服务端会在状态中提供完整图片数据，浏览器可将原图保存到手机。关闭时默认只同步缩略图，减少局域网流量。

### 5.3 手机连不上或频繁断开

按以下顺序检查：

- PC 和手机是否在同一网段，路由器是否开启了 AP 隔离/访客网络隔离。
- Windows 防火墙是否允许该端口通过“专用网络”；临时换一个未被占用的端口测试。
- 地址中是否使用了正确的 PC IPv4，而不是 `localhost`。
- PC 端口是否在配置保存后仍处于开启状态；关闭再开启会重新监听。
- 手机浏览器是否缓存了旧的前端资源；重新打开页面或强制刷新。
- 如果出现 `401`，表示会话过期或服务重启，刷新页面让它重新握手。
- 如果出现 `failed to fetch`，先确认服务端仍在运行，再检查防火墙、VPN 和 Wi-Fi 隔离。页面不会用这类瞬态错误覆盖最后一次有效答案。

## 6. 局域网加密说明

手机端不是直接发送明文题图和答案。握手阶段：

1. 手机和 PC 各生成临时 P-256 密钥对。
2. 双方通过 ECDH 计算共享值，并用 SHA-256 派生 AES-256-GCM 会话密钥。
3. 后续状态、答案、截图和控制命令都使用新的随机 12 字节 IV 加密。
4. 服务端会话空闲约 12 小时后失效，刷新页面会重新握手。

当前实现的边界也很重要：静态 HTML/JS 资源和握手元数据仍通过普通 HTTP 提供，协议没有 TLS 证书校验、二维码配对或设备撤销机制。因此它主要防止同一局域网的被动抓包，不适合不可信网络或公网暴露。后续可以在不改 React 业务协议的情况下增加 WSS、一次性配对码和设备令牌。

## 7. 配置参考

| 配置 | 默认值 | 作用 |
| --- | --- | --- |
| Provider | Codex CLI | 选择求解后端；Claude 入口当前为预留 |
| Codex CLI 路径 | 自动查找 | `codex`、`codex.cmd` 或完整路径 |
| Codex 模型 | CLI 默认 | 从 `model/list` 读取候选，也可手动填写 |
| 推理深度 | low | 通过 `model_reasoning_effort` 传递，可选 none/minimal/low/medium/high/xhigh/max/ultra；实际由模型决定 |
| 速度模式 | 标准 1× | `default` 为标准；`priority` 为 Fast，支持的模型/账号约 1.5×，用量可能更高 |
| 超时 | 90 秒 | 有效范围 5–600 秒，超时会杀掉 CLI 子进程树 |
| 浮窗透明度 | 70% | 有效范围 5%–95%，同时调整背景与文字可读性 |
| 浮窗主题 | 实时跟随 | 日间、夜间、实时跟随桌面背景 |
| 浮窗字号 | 18px | 有效范围 10–48px |
| 浮窗宽/高 | 560×360 | 有效范围 240–1400 × 160–1000 |
| 全局 Prompt 附加要求 | 空 | 追加到所有题型 Prompt |
| 局域网控制 | 关闭 | 开启后监听 `0.0.0.0:端口` |
| 局域网端口 | 18765 | 有效范围 1024–65535 |
| 手机保存完整截图 | 关闭 | 开启后才向手机同步原图 |
| 快捷键 | 见上表 | 保存前拒绝重复绑定 |

应用配置、加密历史和临时截图由 Tauri 应用数据目录管理。任务完成、取消或异常时会删除临时题图；历史图片和答案使用 AES-256-GCM 保存，并由当前 Windows 用户的 DPAPI 保护主密钥，默认保留 30 天。

## 8. 常见问题

### “cargo metadata” 或 “cargo 不是内部或外部命令”

重新安装 Rust stable，确认 `%USERPROFILE%\.cargo\bin` 在 PATH 中，然后重新打开终端。不要只安装 Node.js；Tauri 的 Rust 核心必须经过 Cargo 编译。

### VsDevCmd.bat 找不到

在 Visual Studio Installer 中修改 Build Tools，补上 Desktop development with C++、MSVC 和 Windows SDK。安装完成后重新运行 `npm run tauri dev`。

### 求解失败：install and authenticate Codex first

执行 `codex --version` 和 `codex login`。如果项目使用的是 npm 的 `codex.cmd`，把该文件的完整路径填入配置页；应用会处理常见的 Node shim。

### 求解失败：No prompt provided via stdin

这是旧版调用方式的错误。当前实现通过 stdin 向 `codex exec` 发送完整 Prompt；如果仍出现该错误，请确认正在运行最新构建，并检查 CLI 路径没有指向旧脚本。

### 模型候选为空或模型切换无效

模型列表依赖当前 Codex CLI 的 `app-server`、登录状态和账号权限。先在终端登录并更新 CLI；也可以手动输入模型 ID。保存配置后，新任务才会使用新模型，正在运行的任务不会中途切换。

### 为什么没有流式文字或 Markdown

这是当前产品选择：先保证手机断线、窗口保护和最终结果的一致性，UI 只渲染完成后的纯文本答案。Provider 的中间 JSONL 仅用于判断进度，不会转发到浮窗或手机。

### 浮窗仍被录屏或保护状态失败

`WDA_EXCLUDEFROMCAPTURE` 只覆盖支持该属性的 Windows 公有捕获路径。应用会读回保护状态，失败时拒绝展示答案；无法对相机、采集卡、远程桌面和特权录制做保证。

### 快捷键冲突

每个输入框录入时应包含至少一个修饰键。保存前会检查全量重复，错误只显示在冲突的配置项下方。若系统中已有其他软件占用该组合键，请换一个组合；保存成功后旧注册会先注销再注册。

## 9. 设计取舍与扩展方向

### 为什么选择 Tauri + Rust + React

Rust 直接接近 Windows 的截图、窗口和全局快捷键 API，适合做低延迟、可取消的本地任务；Tauri 的运行时体积和内存通常小于打包 Chromium 的方案；React 负责复杂配置表单和手机页面，组件可以复用。代价是 Windows 原生工具链较重，开发者需要同时维护 Rust 和 TypeScript 类型边界。

### 为什么先用本机 CLI

CLI 可以复用用户现有登录态和模型权限，不需要在应用内保存 API Key。缺点是依赖本机安装、登录和命令行版本；未来的 API Provider 可以复用同一份任务结构，在不改变界面的前提下替换执行层。

### 为什么草稿限制 6 张并保留顺序

限制可以控制截图磁盘占用、Prompt 大小和手机同步成本；顺序对连续题目、多页代码和推导题很重要。若未来支持更多图片，可以把草稿拆成分页批次，而不是无上限地扩大一次 CLI 请求。

### 为什么 WebSocket 仍保留轮询降级

WebSocket 适合即时状态，但移动网络会主动回收连接；轮询是可靠的兜底。实现上每个页面只允许一个 WebSocket 和一个轮询任务，连接成功会停止轮询，断线则指数退避重连，避免重复连接和状态闪烁。

### 高频实现问题

- **如何避免截图把答案浮窗截进去？** 截图路径在 Rust 中隐藏/保护窗口并依赖捕获排除属性；保护状态未确认时不展示答案。
- **如何保证取消真的停止？** Rust 保存 CLI 进程 ID，取消或超时使用 Windows 的进程树终止，随后统一清理临时文件。
- **如何加入新 Provider？** 实现统一的 `AnswerProvider` 接口，接收 Prompt、图片引用和运行配置，返回 `AnswerResult`；UI 只增加 Provider 配置，不改草稿和浮窗。
- **为什么手机不默认传原图？** 默认只传缩略图能降低序列化和带宽成本；用户显式开启完整截图保存后才传输原图。
- **加密能否替代身份认证？** 不能。当前 ECDH + AES-GCM 保护机密性和完整性，但没有证书式身份确认；不可信网络仍应使用 WSS、配对码或 VPN。

## 10. 从源码继续开发

前端入口和组件位于 `src/`，手机入口是 `src/mobile_app.tsx`；Rust 业务核心位于 `src-tauri/src/`，主要模块包括：

- `main.rs`：应用状态、Tauri 命令、快捷键和窗口生命周期。
- `codex.rs`：Codex CLI 调用、模型列表和 JSONL 解析。
- `local_server.rs`：手机静态资源、加密 HTTP API 和 WebSocket。
- `overlay.rs`：Windows 前台浮窗样式与捕获保护。
- `presets.rs`：内置及自定义题型 Prompt 预设。

修改后至少运行：

~~~powershell
npm run build
npm run check:rust
npm run test:rust
~~~

这样可以同时覆盖 React 类型检查、Rust 编译和核心单元测试。
