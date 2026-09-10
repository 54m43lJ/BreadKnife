# BreadKnife 项目产品需求文档 (PRD)
**文档版本**: v1.0  
**目标平台**: Arch Linux + Hyprland  
**技术栈**: Rust + GTK4 + gtk4-layer-shell  
**项目名称**: BreadKnife  
**二进制名称**: `bread`
---
## 1. 项目概述与运行模型
### 1.1 核心定位
BreadKnife 是一个专为 Hyprland 设计的 Wayland 任务栏。它不是一个通用框架，而是一个开箱即用的精品应用，旨在提供原生的系统集成体验。
### 1.2 运行模型
- **单进程守护**：主进程为 Daemon，常驻后台，PID 文件位于 `/run/user/$UID/bread.pid`。
- **多窗口实例**：Daemon 维护一个 `HashMap<MonitorID, MainWindow>`，每个活动的显示器对应一个独立的 GTK Window 实例。
- **生命周期**：
  - 启动：扫描当前已连接显示器，生成对应 Window；注册 Hyprland IPC 监听器。
  - 运行：接收 IPC/Socket 事件，事件循环驱动 UI 更新。
  - 退出：接收 Socket `exit` 指令，销毁所有 Window，释放资源，退出进程。
---
## 2. 核心架构设计
### 2.1 技术依赖
- **GUI**: GTK4
- **Wayland 集成**: `gtk4-layer-shell` (用于实现 Layer Surface 锚定)
- **通信**: `zbus` (DBus), Hyprland IPC (Unix Socket)
### 2.2 通信接口定义
**A. Hyprland IPC (只读监听)**
- **路径**: `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock`
- **关键事件**:
  - `monitoradded` / `monitorremoved`: 触发 Window 实例的创建与销毁。
  - `workspace>>X`: 触发 Workspace 模块状态切换。
  - `activewindow`: 触发窗口信息模块更新。
  - `configreloaded`: 可选，触发内部 UI 状态刷新。
**B. Unix Socket (控制接口)**
- **路径**: `/run/user/$UID/bread.sock`
- **协议**: 文本行协议，以 `\n` 结尾。
- **指令集**:
  - `EXIT`: 关闭守护进程。
  - `TOGGLE`: 切换任务栏显示/隐藏状态。
  - `RELOAD`: 热重载配置文件与 CSS 样式。
### 2.3 显示器与布局管理
- **Layer Shell 配置**: 锚点设为 `Top | Left | Right`，强制任务栏宽度等于显示器宽度。
- **事件处理**:
  - **插拔显示器**: 监听 Hyprland IPC 事件，动态 Spawn 或 Destroy Window。
  - **分辨率/旋转变化**: 由 GTK 窗口自动 resize，内部组件根据 Flex 规则自动重排。
---
## 3. 模块功能规格
### 3.1 Workspace 控制模块
- **绑定逻辑 (反向映射)**:
  - 启动时及 Monitor 插拔时，遍历所有 Workspace，根据其 `monitor` 属性反向建立映射表。
  - 遵循“严格预分配”原则：每个显示器固定管理若干连续 Workspace ID，不支持跨显示器移动。
- **UI 状态**:
  - **激活**: 高亮显示 (`bg-active`, `fg-primary`)。
  - **空**: 显示为灰色 (`fg-muted`)，表示该 ID 存在但无窗口。
  - **紧急**: 红色标识 (`danger`)。
- **交互**:
  - 鼠标左键点击：切换至对应 Workspace。
  - 滚轮滚动：在当前显示器的 Workspace 列表中顺序切换。
### 3.2 窗口信息模块
- **信息获取**: 监听 `activewindow` IPC 事件。
- **图标**: 根据窗口 Class 查询系统 Icon Theme，找不到则显示默认图标。
- **标题**:
  - 单行显示，超出宽度截断 (`PANGO_ELLIPSIZE_END`)。
  - Hover 显示完整 Tooltip。
- **通知中心**:
  - 点击标题栏区域打开 GTK Popover。
  - 后台实现 `org.freedesktop.Notifications` DBus 接口，缓存并显示通知列表。
### 3.3 系统托盘模块
- **协议**: 实现 StatusNotifierItem (SNI) 协议，直接与 DBus 交互。
- **功能**:
  - 动态检测应用注册/注销 SNI 服务。
  - 支持图标加载、Tooltip 显示。
  - 支持左键和右键交互（调用 `Activate` / `ContextMenu`）。
  - 自动给图表加高对比度边框或者背景
	  - 智能选择深浅色
### 3.4 控制中心模块
**A. 入口组件**
- **布局**: `[Battery Indicator] [Network Indicator] [Clock]`
- **电池**: DBus `org.freedesktop.UPower`，无电池设备自动隐藏。
- **网络**: DBus `org.freedesktop.NetworkManager`，显示 WiFi/Ethernet 图标。
- **时钟**: Monospace 字体，实时更新。
**B. 弹出面板**
- **媒体控制**: MPRIS DBus 接口，显示播放器信息及控制按钮。
- **音量控制**: 调用 `pamixer` 或 PulseAudio DBus 接口。
- **亮度控制**: 调用 `light` 或读写 `/sys/class/backlight`。
- **网络/WiFi**: 显示状态，Phase 2 实现连接交互。
- **蓝牙**: 显示状态，Phase 2 实现配对交互。
- **日历**: GTK Calendar 组件。
---
## 4. 分期迭代规划
### Phase 1: 核心框架与基础交互 (MVP)
**交付标准**: 稳定运行，核心功能可用。

| 模块 | 功能点 |
| :--- | :--- |
| **架构** | Daemon 生命周期、多显示器支持、Layer Shell 锚定。 |
| **Workspace** | 反向映射、状态显示、点击/滚轮切换。 |
| **Window Info** | 标题显示/截断、图标显示。 |
| **Tray** | 图标显示、基础点击响应。 |
| **Control Center** | 入口显示、面板弹出、音量/亮度/媒体控制基础功能。 |
| **样式** | CSS 加载、基础变量应用。 |
### Phase 2: 完善功能与高级交互
**交付标准**: 完整的用户体验。

| 模块 | 功能点 |
| :--- | :--- |
| **Window Info** | 通知中心列表显示与交互。 |
| **Tray** | 图标状态实时同步、溢出折叠。 |
| **Control Center** | WiFi 列表刷新/连接、蓝牙设备配对、日历交互。 |
| **Config** | Unix Socket `RELOAD` 热重载支持。 |

---
## 5. UI/UX 规格与样式定义
### 5.1 基础变量
```css
/* ── 底板 ── */
@define-color bg_primary #1b1816;
@define-color bg_secondary #24211e;
@define-color bg_hover #302c29;
@define-color bg_active #3d3835;
/* ── 文字 ── */
@define-color fg_primary #ecd9a8;
@define-color fg_secondary #b8a078;
@define-color fg_muted #6b5d48;
/* ── 强调色 ── */
@define-color accent #5c3d7a;
@define-color accent_hover #735294;
/* ── 语义色 ── */
@define-color success #4a9c6c;
@define-color warning #c4954e;
@define-color danger #b8453a;
/* ── 边框 ── */
@define-color border rgba(236, 217, 168, 0.08);
@define-color border_strong rgba(236, 217, 168, 0.15);
```
### 5.2 组件样式规范
- **主窗口**: 背景色 `bg_primary`，底边框 `border_strong`，高度 `30px`。
- **按钮**: 透明背景，圆角 `6px`，Hover 态 `bg_hover`。
- **Workspace 激活**: 背景 `bg_active`，文字加粗。
- **Workspace 空**: 文字色 `fg_muted`，斜体。
- **控制中心入口**: 容器背景 `bg_secondary`，圆角 `6px`。
- **滑块**: 滑轨高度 `4px`，滑块圆形 `12px`，高亮色 `accent`。