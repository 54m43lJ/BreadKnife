# 提示词文档：BreadPan / Boulanger 文档体系生成任务（最终版）
## 第 0 部分：任务说明（给 agent 的开场指令）
为一个 Linux 系统托盘工具链创建四份规格文档。工具链包含两个库 crate（`breadpan`、`boulanger`）和一个下游验收工具（`demo-tray`）。全部文档**中文正文、代码标识符与 API 名英文**。产出只有文档，不写代码。
文档生成的最高准则：
1. **每一条约束都能回答"为什么"**——写不出理由的约束删掉；
2. **实现细节不进正文**——线程拆分、原子性机制等工程决策写入各文档「架构决策记录（ADR）」附录，每条 3–5 行；
3. **示例代码即文档**——所有示例与第 4 部分公开 API 签名逐字一致，标注"作为 doctest 保真维护"；
4. **零下游词汇**——breadpan 与 boulanger 文档中不得出现任务栏/panel/taskbar/BreadKnife 等消费方概念（检验标准：`cargo doc` 页面给从未见过下游项目的人可无障碍使用）；demo-tray 文档不受此限；
5. 图表一律 ASCII 字符画。
---
## 第 1 部分：全局决策记录（所有文档的事实基础，不得偏离）
### 1.1 项目身份
| 项 | 决策 |
|---|---|
| 项目 | `breadpan`（SNI/DBusMenu 托盘客户端库）+ `boulanger`（SNI Watcher 角色库）+ `demo-tray`（人工验收工具） |
| 命名逻辑 | breadpan=烤盘（承放、供取用）；boulanger=法语面包师（照看炉火、收发面团＝登记 Item） |
| 发布状态 | 暂不发布 crates.io；版本策略照常（见 1.9） |
| 使用场景 | Wayland 会话；**强绑定 GTK 框架**（整套项目以 GTK 编写，不做跨框架移植） |
| 目录形态 | 同一 workspace 平行 crate；`breadpan ⊥ boulanger`，互相零依赖 |
### 1.2 breadpan 定位与边界
**是什么**：无头的 SNI 协议客户端库 + DBusMenu 客户端库，以统一事件流交付托盘世界的全部变化；独家支持 GTK，为 GTK 应用提供深度整合（渲染对象直出 + 事件桥接方案）。**不实现 Watcher 角色**（由 boulanger 承担，breadpan 仅作为客户端观察其存亡）。
**Non-goals（每条带理由）**：
| 不做 | 理由 |
|---|---|
| 拥有任何 UI（窗口/布局/事件循环） | 原则 ①：渲染逻辑归消费方；但数据形态深度整合 GTK（见 1.4） |
| XEmbed/X11 传统托盘 | 场景钉死 Wayland |
| 菜单渲染/弹出定位 | 交付 MenuSnapshot 数据模型与激活接口 |
| AttentionMovieName | 实际使用者寥寥，进「已知不实现清单」 |
| 异步运行时泄漏 | 公开 API 零 async；内部用 zbus 自带执行器 |
| 菜单缓存 | 见 1.6：零缓存零状态，一致性代价 >> 缓存收益 |
### 1.3 设计原则（正文逐条附"为什么"）
1. **库不拥有 UI**：无窗口、无布局、无事件循环；**独家支持 GTK 框架，为 GTK 应用提供深度整合**（`tray::gtk::paintable` 直出 GdkPaintable、事件桥接权威方案），渲染逻辑仍归客户端；
2. **阻塞优先**：零 async 泄漏，内部线程模型对使用者不可见；
3. **事件通知 + 主动拉取快照**（不推大对象）；
4. **容错降级**：永不 panic，坏 Item 不拖垮整体，单 Item 死亡隔离；
5. **依赖白名单**：`zbus`（锁 "5"）+ `thiserror` + **`gtk4`** + `std`；
6. **规范化兜底 + raw 逃生门**：对可确定性识别的协议偏差，库有责任转化为规范行为；识别不确定处保持沉默（宁可原样交付，不可错误"纠正"）；所有经过解析/兜底的字段同时保留可选的 raw 原始字段，供客户端特殊处理（例：`MenuItem.raw_label`、`Icon` 双份数据）。
**硬约束**：
- a. workspace 零反向依赖：breadpan/boulanger 的 Cargo.toml 不出现 workspace 内任何成员；CI 隔离目录 `cargo build -p` 验证；
- b. 零下游概念泄漏（GTK 是框架绑定，不算任务栏耦合，见准则 4）。
### 1.4 GTK 深度整合（锁死 GTK 的落地）
- **渲染**：`tray::gtk::paintable(icon, theme, scale)` 自由函数——主题命中 → `GtkIconPaintable`；未命中 → `CanonPixmap` → `gdk::MemoryTexture`（预乘 RGBA8，一次调用成纹理）；Icon 两字段均 None → 返回 None，客户端自定 broken image；overlay/attention/tooltip 图标同类型全覆盖；
- **三个诚实约束写入文档**：① `paintable()` 必须在 GTK 主线程调用（GDK 非线程安全）；② SNI IconPixmap 无 scale 信息，HiDPI 下位图按 1:1 交付由 GTK 绘制时缩放，可能偏糊——协议信息缺失，记入《已知协议偏差》；③ gtk4-rs 锁当前大版本，随下游 GTK 版本升版，不做独立兼容承诺；
- **事件桥接**：两种模式并列（回调 + `MainContext::invoke`；转发线程 + `glib::MainContext::channel`），无权威模式，对照表 + 各自完整示例。
### 1.5 运行模型
**连接时序**：
```
connect(config) / builder().spawn() ← 语义完全相同，connect 是 sugar
├─ 连接会话总线 ← 唯一同步失败点，失败立即 Err(Bus)，不重试
├─ 启动内部线程，立即返回 (TrayHandle, Receiver<TrayEvent>) ← 纯非阻塞
└─ 后台：Watcher 发现
   ├─ 无 Watcher → 500ms 起指数退避（×2）封顶 30s，无限重试
   │   每次到点 → Internal(Info)（含第几次尝试、累计等待）
   ├─ Watcher 出现 → Host 注册 → WatcherChanged(WatcherUp)
   └─ 启动扫描 → 逐个补发存量 Added（仅服务于 connect 的初始通道）
```
**`WatcherState` 语义定义（精确口径，正文必须收录）**：
`WatcherUp` / `WatcherDown` 指 **`org.kde.StatusNotifierWatcher` 总线名在会话总线上的 owner 存在性变迁**（即该 well-known name 当前是否有人持有），仅此含义：不反映库自身连接健康度，不区分任何具体 Watcher 实现，不承诺 owner 的身份或版本。
**运行期 Watcher 生命周期（连接完成后，正文必须完整呈现，与连接时序并列）**：
```
运行期（连接建立后）：
WatcherDown（唯一名 owner 消失，NameOwnerChanged 触发）
  ├─ 投递 WatcherChanged(WatcherDown)
  ├─ 已知 Item 全部保留，跟踪照常
  │   （SNI 属性信号由 Item 直发 Host，不经 Watcher，功能不受影响；见 ADR ⑭）
  ├─ 心跳照常（Item 存活探测与 Watcher 存在性无关）
  └─ Host 侧发现回退至发现重试循环（复用 500ms/×2/封顶 30s 曲线，
      每次到点投递 Internal(Info)，与连接期叙述格式一致）
WatcherUp（唯一名 owner 重新出现）
  ├─ Host 重新注册 → 投递 WatcherChanged(WatcherUp)
  └─ 存量 Item 不重扫、不重放 Added
      （Item 从未被移除，无需重扫；Added 补发语义仅属于连接/订阅的
       初始快照，运行期重放会破坏 1.8 的"快照 + 增量"模型）
```
为什么这样设计：Item 不因 Watcher 消失而移除，因为 SNI 数据通路上 Watcher 不是必经环节，移除会制造重注册风暴与状态丢失；运行期不重放 Added，因为补发是"初始快照"职责的一部分，把它扩展到运行期会同时破坏快照模型的原子性与事件流的单调性。
**三条生命线**（正文显式区分）：
| 信息类型 | 载体 | 例子 |
|---|---|---|
| 机器可读状态变迁 | SNI 事件（`WatcherChanged`/`Added`/…） | Watcher 上线 |
| 人类可读过程叙述 | `Internal(Info/Warn)` | "watcher 未就绪，第 3 次重试，已等待 2.0s" |
| 终止宣告 | `Internal(Fatal)` | 总线断开；此后 handle 全部方法返回 `Err(Stopped)` |
**回调契约**：回调在库内部专用线程串行触发；回调内不得阻塞、不得直接调用 GTK；进 UI 线程由消费方桥接。
**通道契约**：内部以回调原语驱动默认 `std::sync::mpsc`；通道满时丢弃并累计（`dropped_event_count()`），**绝不阻塞内部线程**（通道满时不能投递"通道满"事件——自我矛盾，ADR 记录）。
**线程模型（公开承诺）**：回调串行于单一专用线程；方法线程安全；shutdown 幂等；最后一个 handle drop 自动 shutdown；可多实例并行（无 Fallback 后无总线名争抢，无需仲裁）。
### 1.6 交互范式与菜单零缓存
**分界规则：状态靠推送，控制靠有界同步往返。**
| 调用面 | 性质 | 范式 |
|---|---|---|
| Item 生命周期/属性/菜单内容 | 状态流 | 推送（事件）+ 主动拉快照 |
| `activate` / `secondary_activate` / `scroll` / `menu_activate` | 控制指令 | 同步 fire-and-forget，`Result<(), TrayError>` |
| `menu_about_to_show` | 控制指令 | 同步往返，`Result<(), TrayError>`（不再返回 bool） |
**菜单零缓存定稿**：
- `menu(id)` 每次调用 = 一次全量 `GetLayout` 同步往返（典型 1–5ms，右键时机无感）；
- 库**不主动拉取菜单布局**，无"重拉受影响子树"状态机；对菜单**不储存任何状态**；
- `MenuChanged` = dbusmenu `LayoutUpdated`/`ItemsPropertiesUpdated` 信号**纯转发**（零拉取零缓存）；**客户端行为规范：popup 打开期间收到 MenuChanged → 重调 menu(id) 重建 UI**（最强时效性由此达成）；
- `menu_about_to_show(id, item_id)`：下发 AboutToShow 即返回（无缓存故无刷新返回值）；权威调用序列 = `menu_about_to_show` → `menu(id)`（保留理由见 ADR ⑯）；
- `MenuSnapshot.revision` 为**外部来源**（dbusmenu GetLayout 响应原样透传），保留；
- **`menu_activate` 的 item id 仅在当前 MenuSnapshot 生命周期内有效**（dbusmenu 布局更新后 id 可能复用）——醒目警告写入 API 文档；
- `call_timeout_ms` 默认 **500ms**（推导：人急躁时反复右键 → 挂起须近无感，300ms 即够、留弱硬件/虚拟机余量；写入 ADR 防未来的自己"优化"），交互调用全部受其约束，超时返回 `Err(Protocol)`。
**菜单读取场景论证表**（正文收录，支撑零缓存决策）：
| 场景 | 频率 | 零缓存下的满足方式 |
|---|---|---|
| 右键弹出根菜单（AboutToShow → 取数据） | 唯一高频 UI 场景 | 现场全量拉取，1–5ms |
| 弹出菜单内展开子菜单 | 中 | 同上 |
| 调试观察（tray-inspect menu/dump-json） | 低 | 同上 |
| popup 打开期间 Item 侧推送变更 | 低 | MenuChanged 转发 → 客户端重取 |
| 键盘导航/无障碍遍历 | 罕见 | 同上 |
### 1.7 心跳与幽灵治理
- **三种死亡形态**：进程退出（NameOwnerChanged 事件驱动，零成本）；Watcher 重启后未重注册（**不构成故障**——SNI 属性信号 Item 直发 Host 不经 Watcher，跟踪照常，Item 原样保留，ExternalUp 主动探测取消）；进程活着但服务挂死（唯一真实幽灵，心跳是唯一手段）；
- **心跳机制**：`heartbeat_interval_ms` 默认 **60000**，**0 = 关闭**；到点对全部存量 Item 并发探测（`Properties.Get`，受 `call_timeout_ms` 约束）；**连续 2 个周期失败 → `responsive: false` 并投递 `Changed{[Responsive]}`；恢复 → true**；
- **心跳无移除权**：移除仅由唯一名消失（NameOwnerChanged）触发。用户如何发现无响应应用 = 消费方 UI 职责（置灰/提示，凭 `responsive` 字段）；强制退出不在托盘职责内（SNI 不暴露 pid，协议层无通路；正当归属是 WM/系统监视器）。库的职责边界 = **诚实呈现状态**（现状的坏 UX 是图标看似正常、点击静默失败）。
### 1.8 订阅与快照（D4 定稿）
**形态**：`subscribe() -> (TraySnapshot, Receiver<TrayEvent>)`——**原子配对**。单独的 snapshot() 方法会引入快照与订阅之间的窗口缺口，故一个调用产出两者。
**TraySnapshot 范围严格定义**（写入正文）：
> **包含**：
> 1. `watcher: WatcherState`——订阅时刻 `org.kde.StatusNotifierWatcher` 名字的 owner 存在性（WatcherUp / WatcherDown，语义见 1.5）；
> 2. `items: Vec<TrayItem>`——订阅时刻全部已跟踪 Item 的完整记录（含 responsive、规范化 Icon、tooltip 等全部字段）。
>
> **不包含**：
> 1. 任何菜单数据（零缓存原则，菜单按需 `menu()` 现场拉取）；
> 2. 事件历史（此前发生的事件不重放）；
> 3. 处于 debounce 窗口内尚未合并生效的属性变更（它们将作为 Changed 事件在通道中正常到达）。
>
> **边界语义**：快照 = T 时刻库内最后已应用状态；通道 = 严格 T 之后的事件。无重叠（快照不含 T 后变化）、无缺口（T 后变化全在通道）。实现保证：取状态克隆与注册新 Sender 在同一状态锁临界区内完成，锁外投递（ADR 记录）。
**客户端初始化纪律**（写入正文的使用规范）：
> **快照仅供初始化使用；初始化完成之前不得处理通道消息**。推荐序列：
>
> ```rust
> let (snap, rx) = handle.subscribe();
> // 阶段一：仅用 snap构建初始 UI，禁止 recv()
> render_initial(&snap);
> // 阶段二：初始化完成后开始增量处理
> for ev in rx { apply(ev); }
> ```
>
> 此纪律由客户端遵守（库无法强制）。安全性依据：mpsc 通道有缓冲，初始化期间到达的事件无害积压；快照与通道无重叠无缺口，故"先快照后事件"的串行处理与真实时序一致。初始化期间的变化（如快照后某 Item 立即 Changed）会在阶段二被正确应用——以快照为基准、事件为增量，不存在丢失。
**connect 通道的差异及理由**（文档各写一句防误读）：connect 返回的接收端从 t=0 存在，启动扫描的 Added 一个不漏，故维持纯事件、不配快照；subscribe 的接收端迟于启动，故以快照补齐。
**重放模式废弃理由**（ADR）：逐条重放受背压管辖（通道缓冲小于 Item 数时初始快照被静默截断）；补发是时间区间而非原子时刻（区间内的新变化造成陈旧快照混发）；需每订阅者独立重放缓冲与游标，实现复杂。快照方案三者皆免，拷贝成本为订阅时一次性深拷贝（现实规模亚毫秒级）。
### 1.9 版本与稳定性
- SemVer，破坏性变更定义收窄为**"导致下游消费代码需要修改的变更"**；错误 enum 加 variant、`InternalMessage` 加结构化字段等不算；
- 不设 MSRV 承诺，文档记录当前验证通过的 Rust 版本；
- 公开 API 强制 `#![deny(missing_docs)]`；
- v1.0 冻结条件：全部验收项通过 + 真实桌面会话 72h 无 Fatal 级 Internal。
### 1.10 依赖白名单
`zbus`（锁 "5"，允许 patch）+ `thiserror` + `gtk4`（锁当前大版本，随下游 GTK 升版）+ `std`。**无日志门面**——日志是消费方职责，库经事件流（Internal）汇报自身状态。boulanger 依赖白名单另列（无 gtk4）。
---
## 第 2 部分：文件清单与目录树
```
<workspace root>/
├── README.md            # 一页纸：三 crate 一览 + 依赖图 + 阅读入口
├── breadpan/README.md   # 主规格文档（核心产出，篇幅最大）
├── boulanger/README.md  # Watcher 角色规格
└── demo-tray/README.md  # 人工验收/debug 工具（唯一写两库用法的下游文档）
```
依赖与引用方向（写入根 README 的 ASCII 图）：
```
breadknife (下游，不在本仓库)
├─ 先启动 boulanger 服务，再 breadpan::connect
│
demo-tray ──依赖──> breadpan + boulanger （用法文档仅在此）
```
---
## 第 3 部分：逐文档内容明细表
### 3.1 `README.md`（根，一页纸）
| 章节 | 必写内容 |
|---|---|
| 项目一览 | breadpan（"SNI/DBusMenu 客户端库，GTK 深度整合"）/ boulanger / demo-tray 各一句话定位 |
| 依赖图 | ASCII：两库互相零依赖；demo-tray 单向依赖两库；下游消费方组装两者 |
| 阅读入口 | 指向三份子文档；注明"库文档不含任何下游项目信息" |
| 状态 | v0.x，未发布，接口冻结基线，破坏性变更须先改文档 |
### 3.2 `breadpan/README.md`（主规格）
**§0 30 秒了解**：一句话定位；`minimal.rs` 全文（≤30 行：connect → recv 循环打印事件 → Ctrl-C），标注为 doctest 保真与文档权威示例来源。
**§1 定位与设计原则**：是什么/不是什么（1.2 Non-goals 表）；六条原则（1.3，每条附"为什么"）；硬约束 a/b；原则 ⑥ 附两个实例预告（raw_label、Icon 双份数据）。
**§2 快速上手**
- 2.1 最小示例（§0 同源）；
- 2.2 GTK 事件桥接：两种模式对照表 + 各自完整代码；
- 2.3 GTK 深度整合（渲染）：`tray::gtk::paintable` 用法、主线程约束、主题未命中回退路径、broken image 处理建议。
**§3 架构概览**：ASCII 图（消费方线程 / breadpan 内部线程 / session bus / Watcher）；连接时序图（1.5）；**运行期 Watcher 生命周期状态机（1.5，与连接时序并列成图）**；重试曲线（500ms/×2/封顶 30s/无限；理由：线性递增在 Watcher 长期缺席时变成无意义总线流量）；线程与生命周期公开承诺。
**§4 公开 API 参考**（签名以第 4 部分为准，逐字一致）
| 小节 | 必写要点 |
|---|---|
| 4.1 入口与配置 | `TrayConfig` 五字段默认值与语义（timeout_ms=500 发现重试基准；debounce_ms=50；menu_max_depth=8；call_timeout_ms=500 交互上界；heartbeat_interval_ms=60000，0=关闭）；connect 与 builder().spawn() 语义相同、均纯非阻塞 |
| 4.2 TrayHandle | 全方法语义；`subscribe()` 原子配对契约 + **TraySnapshot 范围严格定义 + 客户端初始化纪律**（1.8 全文收录，醒目排版）；`dropped_event_count()`；`shutdown()` 幂等；最后一个 handle drop 自动 shutdown |
| 4.3 数据模型 | `TrayItemId`（bus_name+path；同应用重启=不同 Item；**跨重启稳定关联推荐用 SNI `Id` 属性**——否则每个使用者都会重踩）；`TrayItem` 全字段（含 `responsive` 语义：只标记不移除，移除权专属 NameOwnerChanged）；`Icon` 双交付 + `CanonPixmap`（预乘 RGBA8；承诺口径：合规输入必正确，不合规输入按规范透传，见《技术兜底》）；`ToolTip`；`WatcherState` 语义（1.5 精确口径） |
| 4.4 菜单模型 | `MenuSnapshot`/`MenuItem`（`label` 已解析 + `mnemonic_index: Option<usize>` + `raw_label` 逃生门，`__`→`_`、`_x`→`x` 规则写明）；零缓存语义全表（1.6）；`menu_about_to_show` 权威序列；**menu item id 生命周期醒目警告**；revision 外部来源说明 |
**§5 事件语义**
- `TrayEvent` 全变体投递语义表：同一 Item 严格有序/全局跨 Item 不保证；去抖合并（debounce 窗口内 PropertiesChanged 合并为一次 Changed，changed 字段取并集）；启动扫描补发（仅 connect 通道）；背压丢弃 + 计数器；`Internal(Fatal)` 终止后全部方法返回 `Stopped`；
- `WatcherChanged` 语义：仅 `WatcherUp`/`WatcherDown`（1.5 精确口径 + 运行期生命周期：WatcherDown 保留 Item、回退发现循环；WatcherUp 重新注册 Host、不重扫不重放）；
- `MenuChanged` 纯转发语义 + 客户端 popup 重取规范；
- `Changed` 不携带快照（id + changed 字段列表，消费方 `item(id)` 拉取——与快照哲学同构）；
- 心跳触发的 `Changed{[Responsive]}` 语义。
**§6 协议实现说明**
- 6.1 SNI 覆盖范围：全部属性清单（含 ToolTip、ItemIsMenu）、交互调用；已知不实现清单（AttentionMovieName、XEmbed）；
- 6.2 DBusMenu 覆盖范围：全量布局拉取、深度截断、AboutToShow 策略、信号转发；
- 6.3 **已知协议偏差与不保证清单**：IconName 主题查不到（信息损失由 Icon 双交付消解）；HiDPI 位图无 scale 信息；dbusmenu label mnemonic 约定；
- 6.4 **技术兜底**（独立一节）：① Pixmap 字节序——两类偏差（host/LE 打包、straight alpha）的现象与肉眼识别方法（颜色偏蓝/透明度错乱），**库不做启发式纠偏**（误判成本单向有害；启发式对不透明图标失去分辨力）；`CanonPixmap` 承诺口径；② mnemonic 解析（确定性，符合原则 ⑥ 所以做）；③ 心跳作为挂死检测的兜底（2-strike 保守方向）；
- 6.5 Host 注册细节：唯一名 `_org.kde.StatusNotifierHost.<pid>-<序号>`，同 pid 多实例序号分配规则（进程内原子计数器）；**运行期 WatcherUp 时的 Host 重新注册（1.5 生命周期）**。
**§7 测试与脚手架**
- 7.1 `testing` 特性：`tray::testing::spawn_demo_item(config) -> DemoItem`（Drop 自动注销）、`tray::testing::DemoWatcher`。**DemoWatcher 功能集钉死为以下三项（闭集，替身只服务 CI 断言，不追求协议完整）**：
  1. `take()` / `release()`：控制 `org.kde.StatusNotifierWatcher` 总线名持有/释放，用于对运行中的 Host 制造 `WatcherChanged(WatcherUp/WatcherDown)` 变迁（Drop 自动 release）；
  2. `registered_items()`：返回其簿记中已登记的 Item 唯一名列表，供测试断言 Item 注册确实发生；
  3. Host 注册事件计数：记录 RegisterStatusNotifierHost 调用次数，供测试断言 Host 注册（含运行期 WatcherUp 后的重注册）确实发生。
  **定位声明：测试替身而非生产实现，与 boulanger 不构成重复**——替身保证 breadpan 在裸会话下的 CI 自足性；
- 7.2 脚手架：`tray-inspect`（list / watch / menu / activate / dump-json，逐条验收标准）；`demo-item`（stdin 指令集逐条列出）；`minimal.rs`；`demo-watcher-switch`（回车切换 Watcher 名字持有/释放，制造 WatcherChanged 变迁）；
- 7.3 CI 门禁：无显示服务环境通过；集成测试固定跑在私有 `dbus-run-session`；仅需 dbus session。
**§8 错误模型**：四变体：`Bus(String)` / `Protocol(String)` / `ItemNotFound(String)` / `Stopped`；容错总则三条（不 panic；单 Item死亡隔离；总线断开 → Internal(Fatal) → Stopped，由消费方决定重建）。
**§9 验收标准**：单元测试（事件去抖合并、`TrayItemId` 相等语义、mnemonic 解析、心跳 2-strike状态机）；集成测试（同进程 spawn_demo_item 全事件序列断言：注册→属性变更→MenuChanged 转发→注销；**运行期 Watcher 生命周期断言：经 DemoWatcher take/release 制造 WatcherDown→WatcherUp，断言 Item 保留、Host 重新注册、全程事件序列符合 1.5 生命周期定义**）；互操作验收（nm-applet、blueman-applet）；无 Watcher 验收（裸会话下 Internal Info 重试序列可见、demo-item 注册后正常）；快照-通道边界验收（subscribe 后立即变更属性，断言快照为旧值且 Changed 事件在通道中到达）。
**附录 A：ADR**（每条 3–5 行，含背景/决策/理由）：
① 定时器线程独立（zbus 阻塞迭代器无超时能力）；
② Fallback Watcher 移除（生产路径死代码 + 总线名争抢，Watch 角色归 boulanger，多实例仲裁问题随之蒸发）；
③ 重试曲线 500ms/×2/30s 推导；
④ call_timeout_ms=500ms 推导；
⑤ 事件不携带快照；
⑥ 通道满用计数器而非事件（自指矛盾）；
⑦ InternalMessage 以 String 起步（加字段非破坏性）；
⑧ Fatal 并入 Internal(Fatal)（生命线分离）；
⑨ Timeout 错误变体删除（错误是方法结果，重试叙述是事件）；
⑩ Pixmap 不做启发式纠偏（误判单向有害 + 启发式对不透明图标失效）；
⑪ 菜单零缓存（一致性代价 >> 缓存收益；场景论证表收编）；
⑫ subscribe 原子配对与重放模式废弃（截断/陈旧窗口/复杂度三因）；
⑬ 心跳 2-strike 与 60s 默认值推导（死亡感知 ≤2 分钟 vs 总线流量可忽略；只标记不移除的保守方向）；
⑭ ExternalUp 探测取消（属性信号不经 Watcher，未重注册 Item 功能完好）；
⑮ gtk4 入白名单（框架绑定定位声明；主线程约束、HiDPI 局限、版本策略）；
⑯ **零缓存下保留 `menu_about_to_show` 权威序列**（背景：dbusmenu 规范要求 Item 在布局将变时发 LayoutUpdated/ItemsPropertiesUpdated，但真实生态中部分 Item 从不发这些信号，客户端若只依赖信号重取将弹出不新鲜菜单；决策：AboutToShow 作为弹出前强制下发步骤保留在权威序列中，它同时是规范机制的兜底与不合规 Item 的唯一刷新契机；理由：调用成本为一次微小同步往返，在 1–5ms 的菜单拉取路径上不可感知，保留它是用近零成本换取生态兼容上限）。
**附录 B：版本与稳定性**（按 1.9）。
### 3.3 `boulanger/README.md`
| 章节 | 必写内容 |
|---|---|
| 定位 | SNI Watcher 角色的纯服务端小库；无 UI；运行形态 = 独立 crate、由消费方进程内组装（留后门：加 main() 即可独立常驻进程——Watcher 活得比面板久可避免面板重启引发的 Item 重注册风暴）；规模预期 300–500 行；依赖白名单：zbus + thiserror + std（无 gtk4） |
| 与 breadpan 的正交性 | 显式声明：互不依赖；breadpan 的 DemoWatcher 是自带测试替身，不构成重复 |
| 协议面 | 持有 `org.kde.StatusNotifierWatcher` 总线名；4 方法（RegisterStatusNotifierItem/Host、IsStatusNotifierHostRegistered、RegisteredStatusNotifierItems）；2 信号（ItemRegistered/Unregistered）；NameOwnerChanged 时清理死名字簿记；名字带 AllowReplacement |
| 行为表 | Item/Host 注册与注销；唯一名消失清理；AllowReplacement 让位语义 |
| 组装示例 | 消费方进程内"先 boulanger 服务、再 breadpan::connect"时序代码草图（不点名具体下游项目） |
| 验收 | dbus-run-session 内独立测试；与 breadpan 集成测试联动 |
| ADR 附录 | 独立 crate 理由；同进程组装 vs 独立常驻进程取舍 |
### 3.4 `demo-tray/README.md`
| 章节 | 必写内容 |
|---|---|
| 身份声明 | 外围人工验收与调试工具；本仓库内**唯一**展示两库用法的文档；不是库文档的一部分 |
| 功能 | GTK4 简单窗口：卡片渲染（**经 `tray::gtk::paintable`**，体现深度整合）；左键 activate（含 item_is_menu 时的菜单行为）、滚轮 scroll、右键按 `menu_about_to_show` → `menu()` → 弹出菜单、点击条目 `menu_activate`、popup 打开期间 MenuChanged 重取；底部事件日志（含 Internal 消息展示，体现"客户端自行管理库叙述流"）；`responsive=false` 的 Item 置灰呈现（示范诚实状态呈现）；内置测试台：spawn demo item 驱动全部变化、take/release watcher 角色 |
| 运行 | `cargo run -p demo-tray` |
| 验收 | 纯 Hyprland（无任何托盘环境）下从窗口内复现全部事件类型 |
---
## 第 4 部分：API 签名定稿草图（agent 逐字对齐基准）
```rust
pub struct TrayConfig {
    pub timeout_ms: u64,            // Watcher 发现重试基准，默认 500，×2 封顶 30s
    pub debounce_ms: u64,           // 属性变更合并窗口，默认 50
    pub menu_max_depth: u8,         // 默认 8
    pub call_timeout_ms: u64,       // 交互调用内部上界，默认 500
    pub heartbeat_interval_ms: u64  // 心跳周期，默认 60000，0 = 关闭
}
impl Tray {
    /// 非阻塞。连接会话总线（失败立即 Err(Bus)），后台执行 Watcher 发现与 Host 注册。
    pub fn connect(config: TrayConfig)
        -> Result<(TrayHandle, Receiver<TrayEvent>), TrayError>;
    pub fn builder(config: TrayConfig) -> TrayBuilder;
    // on_event() 若干次后 spawn()，返回值同上
}
/// 订阅时刻的完整状态。纯值对象，深拷贝，与库内部状态无共享。
/// 范围严格定义见文档 §4.2：仅 watcher 状态 + 全部 Item；无菜单、无事件历史、
/// 不含 debounce 窗口内未生效的变更（它们将在通道中以 Changed 到达）。
pub struct TraySnapshot {
    pub watcher: WatcherState,
    pub items: Vec<TrayItem>,
}
impl TrayHandle {   // Clone + Send + Sync + 'static
    pub fn items(&self) -> Vec<TrayItem>;
    pub fn item(&self, id: &TrayItemId) -> Option<TrayItem>;
    /// 每次调用 = 一次全量 GetLayout 同步往返；库无菜单缓存。
    pub fn menu(&self, id: &TrayItemId) -> Option<MenuSnapshot>;
    // 交互：同步，受 call_timeout_ms 约束
    pub fn activate(&self, id: &TrayItemId, x: i32, y: i32) -> Result<(), TrayError>;
    pub fn secondary_activate(&self, id: &TrayItemId, x: i32, y: i32)
        -> Result<(), TrayError>;
    pub fn scroll(&self, id: &TrayItemId, delta: i32, orient: Orientation)
        -> Result<(), TrayError>;
    /// 触发菜单条目。item_id 仅在当前 MenuSnapshot 生命周期内有效（见 §4.4 警告）。
    pub fn menu_activate(&self, id: &TrayItemId, menu_item_id: i32)
        -> Result<(), TrayError>;
    /// 权威序列：先调用本方法，随后 menu() 现场拉取。
    pub fn menu_about_to_show(&self, id: &TrayItemId, menu_item_id: i32)
        -> Result<(), TrayError>;
    /// 原子配对：返回订阅时刻完整快照 + 仅承载订阅之后事件的通道。
    /// 客户端纪律：快照仅供初始化，初始化完成前不得处理通道消息（§4.2）。
    pub fn subscribe(&self) -> (TraySnapshot, Receiver<TrayEvent>);
    pub fn dropped_event_count(&self) -> u64;
    /// 幂等；最后一个 handle drop 自动触发
    pub fn shutdown(&self);
}
pub enum TrayEvent {
    Added(TrayItem),
    Removed(TrayItemId),
    Changed { id: TrayItemId, changed: Vec<ItemField> }, // 无快照，消费方 item(id) 拉取
    MenuChanged(TrayItemId),   // dbusmenu 信号纯转发；客户端 popup 期间重取 menu(id)
    WatcherChanged(WatcherState), // 仅 WatcherUp / WatcherDown（语义见 §4.3 / 1.5）
    Internal(InternalMessage),    // Fatal 级 = 终止
}
pub struct InternalMessage { pub level: InternalLevel, pub message: String }
pub enum InternalLevel { Info, Warn, Fatal }
pub enum TrayError {
    Bus(String),
    Protocol(String),
    ItemNotFound(String),
    Stopped
}
pub enum WatcherState { WatcherUp, WatcherDown }
pub enum ItemField {
    Status, Title, Description, Icon, OverlayIcon, AttentionIcon,
    Tooltip, WindowId, Category, Responsive,
}
pub struct TrayItem {
    pub id: TrayItemId,           // bus_name + path；跨重启关联推荐 SNI Id 属性
    pub category: TrayCategory,   // Application / Communications / SystemServices / Hardware
    pub status: TrayStatus,       // Passive / Active / NeedsAttention
    pub title: Option<String>,
    pub description: Option<String>, // SNI "Id" 属性
    pub window_id: i32,
    pub item_is_menu: bool,       // 左键亦应弹菜单而非 Activate，客户端自行决定
    pub icon: Icon,               // 双交付
    pub overlay_icon: Option<Icon>,
    pub attention_icon: Option<Icon>,
    pub tooltip: Option<ToolTip>,
    /// 心跳探测结果。连续 2 周期无响应 → false；恢复 → true。
    /// 移除仅由唯一名消失触发，心跳无此权力。
    pub responsive: bool,
}
/// 图标双交付：name 与 rgba 独立可缺。推荐路径：name 查主题 → 未命中用 rgba。
pub struct Icon {
    pub name: Option<String>,
    pub rgba: Option<CanonPixmap>,
}
/// 规范化位图：RGBA8，预乘 alpha，本机字节序，stride = width * 4。
/// 保证：规范合规的 SNI 数据经本库 → 此格式即正确；
/// 不合规输入按规范解释透传（见《技术兜底》），不做启发式纠偏。
pub struct CanonPixmap {
    pub width: u32,
    pub height: u32,
    pub premultiplied_rgba: Vec<u8>,
}
pub struct ToolTip {
    pub icon: Option<Icon>,
    pub title: String,
    pub description: String,
}
pub struct MenuSnapshot {
    pub revision: u32,       // 外部来源：dbusmenu GetLayout 响应原样透传
    pub root: Vec<MenuItem>, // 深度截断于 menu_max_depth
}
pub struct MenuItem {
    pub id: i32,                    // 仅当前 MenuSnapshot 生命周期内有效
    pub kind: MenuItemKind,         // Standard / Separator / Submenu
    pub label: String,              // 已解析：__ → _，_x → x
    pub mnemonic_index: Option<usize>, // 助记符位置，None = 无
    pub raw_label: String,          // dbusmenu 原始字符串（原则 ⑥ 逃生门）
    pub enabled: bool,
    pub visible: bool,
    pub toggled: Option<bool>,
    pub icon: Option<Icon>,
    pub children: Vec<MenuItem>,
}
pub enum MenuItemKind { Standard, Separator, Submenu }
pub enum Orientation { Horizontal, Vertical }
/// GTK 深度整合（必须在 GTK 主线程调用）。
/// 主题命中 → GtkIconPaintable；未命中 → CanonPixmap → MemoryTexture；
/// 两字段均 None → None。
pub mod gtk {
    pub fn paintable(icon: &Icon, theme: &gtk4::IconTheme, scale: i32)
        -> Option<gtk4::gdk::Paintable>;
}
```
---
## 第 5 部分：agent 硬约束与完成定义
**硬约束**：
1. 中文正文、API/代码英文；2. 每条约束附"为什么"；3. 实现细节只进 ADR 附录；4. 代码示例与第 4 部分签名逐字一致并标注 doctest 保真；5. breadpan/boulanger 文档零下游词汇（生成后自查关键词：taskbar/panel/dock/BreadKnife/任务栏/面板）；6. ASCII 图表；7. 引用方向单向；8. 各文档间不复制内容，差异点互相引用。
**完成定义（agent 自检清单）**：
- [ ] 四份文件齐全，路径与第 2 部分一致
- [ ] 第 1 部分每条决策在对应文档中可定位到落点
- [ ] 第 4 部分 API 草图与 breadpan 文档 §4 完全一致（含 TraySnapshot、Icon 双交付、responsive、heartbeat_interval_ms、ItemField::Responsive、menu_about_to_show 签名、tray::gtk::paintable、WatcherState::{WatcherUp, WatcherDown}）
- [ ] 原 SPEC 的全部修订点已体现且无残留旧语义：FallbackPolicy / ~~FallbackActive~~ / InteractionResult / Fatal(String) / TrayError::Timeout / has_menu / Changed 携带快照 / IconSource 二选一 / 菜单缓存与"重拉受影响子树" / menu_about_to_show 返回 bool / ExternalUp、ExternalDown 旧命名——均为删除或替换后状态
- [ ] 三条生命线分工表、两种 GTK 桥接模式对照表、交互范式论证表、菜单读取场景论证表、**运行期 Watcher 生命周期状态机**均已收录
- [ ] TraySnapshot 范围严格定义 + 客户端初始化纪律（"初始化完成前不得处理通道消息"）醒目呈现于 §4.2
- [ ] 《技术兜底》独立成节（§6.4），含 Pixmap 偏差"仅记录不纠偏"口径与 CanonPixmap 承诺
- [ ] menu item id 生命周期警告、SNI Id 稳定键建议、主线程约束、HiDPI 局限均已写入
- [ ] **DemoWatcher 功能集三项闭集（take/release、registered_items、Host 注册计数）写入 §7.1**
- [ ] ADR 附录覆盖第 3.2 节列出的 ①–⑯ 全部条目
- [ ] 全部产出文档通过零下游词汇自查，无待定决策残留字样