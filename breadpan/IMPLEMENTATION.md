# breadpan 实现记录

> 定位：ARCHITECTURE / CODE 两档**未覆盖**内容的补充记录——协议生态的实测怪癖、
> 文档契约留下缺口处的补全决策、落地时受技术栈约束的绕行方式。
> 权威边界不变：公开 API、事件与错误语义以
> [ARCHITECTURE.html §3](ARCHITECTURE.html#sec-3) 为准；内部骨架以
> [CODE.html](CODE.html) 为准。本文与二者冲突时，以二者为准。

## 1. 协议生态实测发现（文档未定义的输入形态）

实测环境：无 Watcher 的裸 Wayland 桌面 + 真实应用（QQ 音乐、clash-verge）。

| # | 现象 | 实例 | 本库处理 |
|---|------|------|----------|
| 1 | `RegisterStatusNotifierItem` 参数存在**三种形态**：规范服务名；`"name/path"` 混串；Ayatana 风格**纯对象路径** | clash-verge 传 `/org/ayatana/NotificationItem/...` | 前两种就地解析；纯路径形态取**调用方唯一名**作服务名（经消息头 sender 提取，KDE Watcher 的既定语义）；无 sender 或空串 → 拒绝受理（Protocol） |
| 2 | 存量 Item 可能**不实现 Introspect**（返回空 XML） | QQ 音乐（Electron/CEF 内核） | 反向发现探测双级：Introspect 接口名确认 → 失败则 `Properties.GetAll` 直接属性读取确认 |
| 3 | Item 可能以**唯一名**注册（无 well-known 名） | clash-verge（`:1.22`） | 唯一名即 TrayItemId；`GetNameOwner` 解析属主（唯一名对自身解析成功） |
| 4 | `IconName` 可能是**文件路径**而非主题图标名 | clash-verge（`/run/user/1000/tray-icon/*.png`） | `IconSource::Name` 忠实透传；渲染直出层主题查不到该"名"→ 回退 `image-missing`（不加载文件，见 §6） |
| 5 | 同一对象可能**双重注册**（规范名 + 路径形态各一次） | mock 复现（真实应用为降级重试） | 按 Watcher 事实行为入两册、两个 id 各自独立；信号落点（Changed/MenuChanged/路由更新）匹配首个命中条目，属主消失时一并移除 |

## 2. 文档契约缺口的补全决策

ARCHITECTURE 规定了"首事件即快照、无重叠无缺口"，但未定义启动竞态的处理；以下为补全：

- **名字申请时序**：bootstrap 顺序为 挂载协议面 → 订阅信号/周期任务 → 反向发现
  → 快照 → **最后申请 Watcher 名**。名字在位前报到无从发生，所有报到天然落在
  快照之后、以增量事件衔接。（若按 CODE 伪码"先申请名"，等待 Watcher 出现的
  Item——如 clash-verge——其 `Added` 会抢在快照之前，实测复现。）
- **快照栅栏**（`snapshot_done`，防御性）：快照投递前置位；栅栏前的报到静默入册
  （由快照承载，不发 `Added`）；启动期 Item 死亡的 `Removed` 进缓冲，快照投递后
  仅对**快照内**的 id 补投（快照外的按从未出现丢弃）；`Changed`/`MenuChanged`
  在栅栏前不投递（节流窗跳过 / dbusmenu 信号丢弃）。
- **信号归属匹配**：New\*/PropertiesChanged/dbusmenu 信号的发送者是**唯一名**，
  注册参数可能是 well-known 名 → 簿记最小集在文档字段（唯一名+对象路径+alive+
  menu+menu_only）之外，还需存**属主唯一名**（注册时 `GetNameOwner` 解析）。
- **同名换主**（`NameOwnerChanged(name, old≠"", new≠"")`）：视为旧条目移除
  （`Removed`）+ 新属主重新入册（`Added`）——应用重启直接顶名的竞态。
- **Watcher 名自身的 NOC**：停机路径主动释放名也触发 NOC，以 `stopping` 标记
  区分——主动释放不产生 `Displaced`，仅异常丢失（持有期不可被替换）才终局。
- **总线断连 → Fatal**：无现成"连接断开"回调；心跳每拍以 `GetNameOwner(自身
  唯一名)` 探测总线，IO 类错误（`InputOutput`/`Handshake`/`Connection`）上报
  Fatal，超时视为繁忙跳过。
- **双前缀等价的实现形态**：`org.kde` 与 `org.freedesktop` 两个接口名挂同一
  路径、同一实现体；**信号广播只走 org.kde**（事实规范前缀，避免双订阅方收到
  重复通告）。
- **宽容取值**：规范外波动不 panic、按缺省处理——`WindowId` 以 I32 或 U32 送达
  均可解析；未知 `Category`/`Status` 回退缺省枚举；空 `Title` → `None`；
  `Menu` 属性以对象路径或字符串送达均可。

## 3. 关键落地细节（CODE 骨架未展开的取舍）

- **同步桥**：CODE 写"同步方法经 rt.block_on 受理"，实际**不可行**——CurrentThread
  运行时由内部线程驱动，跨线程并发 `block_on` 同一运行时属未定义行为。
  实现为 `Handle::spawn`（命令 future）+ `std::sync::mpsc` 阻塞等待。
- **等待通道选 std 而非 tokio**：tokio `oneshot::Receiver::blocking_recv` 在
  消费方自己的异步上下文里调用会 panic；std 通道的 `recv_timeout` 无此限制
  （公开面承诺任意线程可调）。
- **超时层次**：每个 DBus 调用独立受 `timeout_ms` 约束；同步桥另设守护上限
  （4×timeout + 10s，防内部意外悬挂）；`Tray::start` 引导总时限
  `max(10×timeout, 10s)`。守护超时若伴随已停机则归一为 `Stopped`。
- **全局信号订阅**（6 条 match rule，对照 CODE 的"逐 Item 订阅"）：
  NameOwnerChanged、New\*×2 前缀、PropertiesChanged（arg0 限定）×2 前缀、
  dbusmenu。按 (sender 唯一名, path) 对照簿记过滤。免去逐 Item 加/删规则的
  簿记与竞态；代价是总线上未跟踪对象的信号被空过滤（可忽略）。
- **逐调用前缀回退**：属性读取（GetAll）与交互调用（Activate/Scroll 等）先
  `org.kde` 失败再 `org.freedesktop`——兼容只导出规范原文拼写的少数实现。
- **菜单属性合并**：GetLayout 的布局内嵌属性为底，GetGroupProperties 的组属性
  覆盖之（专属性查询为权威来源）。
- **VOID 交互仍等待回执**：VOID 指无返回值，不指无应答——受理即返回
  `Dispatched`，应答/错误以 `InteractionResult` 事件交付（区分"送达"与
  "目标消失"）。
- **dbusmenu Event 参数**：`data` 变体发 `I32(0)`（服务端忽略），时间戳取
  Unix 毫秒截断 u32。
- **滚动符号**：up/left 为负、down/right 为正，steps 缺省 1 逐事件发送。
- **事件通道**：容量 256（库内定值）；满则丢弃并计数（有损交付）；关闭 =
  丢弃 Sender（消费端 `recv()` → `None`）。
- **心跳**：周期 5s（库内定值）；每 Item 探测 = `Get(Status)`；结论只写簿记
  `alive`，不产生事件。
- **停机次序**：先置 `Stopped`（此后 Handle 方法一律 Stopped）再置 `stopping`
  （名释放的 NOC 不触发 Displaced），最后信号停机并 join 内部线程；运行时线程
  自身调用 shutdown 时不 join（防自 join 死锁）。
- **自动停机的前提**：内部任务只持有子 Arc（tracker/events/pending/…），
  **不持有** `Arc<RuntimeShared>`——最后一个 Handle drop 时 strong_count==1
  才能触发 shutdown。
- **快照内排序**：按 id 排序投递（发现序不稳定，排序保证确定性输出）。

## 4. 技术栈约束与绕行（zbus 5 / tokio 1 / zvariant 5 / gtk4-rs 0.10）

| 约束 | 绕行 |
|------|------|
| CurrentThread 运行时跨线程并发 `block_on` 未定义 | 同步桥 spawn+std 通道（§3） |
| 依赖白名单不含 futures；MessageStream 需 Stream trait 消费 | `zbus::export::futures_core::Stream` + `std::future::poll_fn` + `std::pin::pin!` 手写 `next` |
| tokio mpsc Receiver 无 `recv_timeout` | try_recv 轮询 + 截止时间（start 等待与测试工具） |
| tokio mpsc/oneshot 的 Sender **非 Sync**（破坏 TrayHandle 契约） | 内部上报通道全部用 watch（Send+Sync） |
| zvariant `OwnedValue` 无 tuple 的 blanket 转换 | 复合类型经 `Value::from(OwnedValue)` 再 `try_into`（tuple 的 `TryFrom<Value>` 存在） |
| `OwnedValue` 是包装结构而非枚举，不能按变体匹配 | 经 `Deref` 到 `Value` 再匹配标量变体 |
| `StructureBuilder` 手工构造的结构回转 tuple 报 `IncorrectType`（实测）；`From<tuple> for Structure` 可靠 | 测试 mock 的布局节点一律用 `From<tuple>` 构造 |
| 名字申请必须走 `Connection` API（raw RequestName 会使 ObjectServer 不路由该名的入站调用） | `request_name_with_flags`，flags 为 enumflags2 `BitFlags`（`DoNotQueue.into()`） |
| zbus 警告：先申请名后挂协议面会丢早到调用 | 挂面 → 快照 → 申请名（与 §2 时序决策一致） |
| `MessageStream` 必须持续 poll，drop 时自动移除 match rule | 每条规则一个常驻任务；订阅失败降级为 Narration |
| 本库 `gtk` 模块名遮蔽 gtk4 crate 的 extern 名（E0463，`::gtk` 亦不可用） | Cargo 依赖重命名 `gtk_crate = { package = "gtk4" }` |
| SNI ARGB32 为网络字节序（字节序 [A,R,G,B]）、未预乘 | 对应 `gdk::MemoryFormat::A8r8g8b8`（非预乘变体），字节序天然对齐、无需换序 |
| `IconTheme` 逐 Display 取用且须 GTK 主线程 | `IconTheme::for_display` + 文档标注主线程约束；主题未命中 → `image-missing`；位图尺寸不完整 → 1×1 透明纹理（不 panic） |
| `call` 的 body 参数需 Sized（`&str` 会使 B=str） | GetAll 等字符串体临时 `to_string()`（一次性小分配） |

## 5. 测试设计

- **Watcher 名全总线唯一** → 生命周期测试必须串行：全部阶段收拢进单个
  `#[test]`，避免并行抢名。
- **mock 架构**：mock Item / mock dbusmenu 以独立的 CurrentThread 运行时线程
  驱动（`handle.spawn` + std 通道桥，与库内同步桥同构），与被测库同进程同总线。
- **总线隔离**：`dbus-run-session -- cargo test` 全量运行；真实桌面已有他人
  Watcher 时生命周期用例**跳过**（不误报）；单元测试均为纯函数，任意环境可跑。
- **真实总线容忍**：真实会话上快照可能含其他真实 Item——断言一律按 id 过滤，
  不假设快照为空；fire-and-forget 回执以轮询等待（`wait_until`/`recv_matching`）。
- **双注册落点**：同一对象双 id 时 Changed/MenuChanged/路由更新的落点 id 不确定
  （HashMap 序）——断言接受任一 id，路由断言捕获实际落点。

## 6. 已知限制与后续方向

- **文件路径图标**：`IconName` 为文件路径时（Ayatana 生态）渲染直出回退
  `image-missing`；如需支持可在此路径加载文件位图（属渲染层增强，协议层已
  忠实透传）。
- **双注册双 id**：与真实 Watcher 行为一致，但消费方会看到两张卡片指向同一
  对象；如需合并可在登记层按 (属主唯一名, 对象路径) 去重（语义取舍待定）。
- **总线守护重启的 Displaced**：依赖 NOC/心跳检测，未在实测中复现（需杀
  dbus-daemon）；停机路径对名释放已做 NOC 区分。
- **发现深度**：反向发现只探测 `/StatusNotifierItem`（规范默认路径）；
  对象路径非默认且未主动报到的 Item 无法发现（规范未提供路径查询手段）。
