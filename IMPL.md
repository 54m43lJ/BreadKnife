# IMPL — 开发过程记录

> 基于 `dev/evol-bnote` 分支、文档基线 `ee16a0d` 的实现过程整理。
> 每条只记录一个主题；同一事实不在不同大块重复。

## 一、开发中发现文档的错误

### 1. bnote `report_fatal` 的同步签名与调用栈互相矛盾

1. 问题描述
   `bnote/CORE.html` §4.5 把 `Bus::report_fatal` 写成同步 `pub(crate) fn`，但 §5.8 的调用栈明确要求 `send_fatal(...).await` 之后再置停机标记。按 §4.5 实现无法等待 `Fatal` 入队；按 §5.8 实现又不符合 §4.5 的签名。
2. 解决方法
   - 以 §4.5 的同步签名为准，方法内部 `rt.spawn` 一个任务，按 `send_fatal().await → claim_stop() → events.close()` 顺序执行。
   - 调用方（`watch_receiver` / `monitor`）不再 `.await`，避免在信号监视路径上阻塞。
   - 命令线程遇到 `Stopped` 时上报 `NotificationStatus::Unavailable`，由通知中心刷新按钮重建会话。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `bnote/CORE.html` | §4.5 `src/bus.rs` | 改 | `report_fatal` 改为 `async fn`，与 §5.8 的 `await` 序列一致；若坚持同步签名，则明确“内部派发任务投递 Fatal”。 |
| `bnote/CORE.html` | §5.8 总线终止 | 不动 | 保留现有 `await` 序列，作为行为契约的权威描述。 |

### 2. breadpan 报到事件顺序与“同一 Item 事件严格有序”冲突

1. 问题描述
   `breadpan/CORE.html` §5.3.1 的报到调用栈是 `iface::emit_registered(service)` 之后再 `events.send(Added(id))`。`emit_registered` 是异步广播，等待期间并发的 `NameChanged` / `Removed` 可能先进入事件队列，导致消费者先看到重键或移除、后看到 `Added`，违反 `USER.html` §4.2 的同一 Item 严格有序。
2. 解决方法
   - 在 `ItemTracker::upsert` 成功插入后立即同步 `events.send(Added)`。
   - 协议面广播改为独立运行时任务，在 `Added` 入队之后执行。
   - 删除后同样先清理 `pending` / `pending_menu`，再投递 `Removed`，避免迟到的 `Changed`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/CORE.html` | §5.3.1 报到 | 改 | 调整顺序为 `upsert → events.send(Added) → 异步 emit_registered`，并注明“事件先入队，广播不得阻塞 Added”。 |
| `breadpan/USER.html` | §4.2 消息与错误 | 不动 | 保留“同一 Item 事件严格有序”的要求，作为上述顺序调整的依据。 |

### 3. 根 `shutdown_files()` 没有覆盖自定义控制套接字

1. 问题描述
   根 `CORE.html` §4.18 只声明 `pub fn shutdown_files();`，而 `BUILD.html` §4 允许 `-s/--socket` 覆盖控制套接字并派生窗口套接字。无参函数只能清理默认路径，自定义路径会残留。
2. 解决方法
   - `protocol::shutdown_files()` 清理默认 PID / 控制 / 窗口套接字。
   - daemon 另按自身跟踪的 `control_path` / `window_path` 再清一次，保证自定义路径也被删除。
   - `Daemon::shutdown_files` 先调用协议默认清理，再做自定义清理。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.18 `shared/protocol.rs` | 改 | 改为 `pub fn shutdown_files(control: &Path);`，按传入控制套接字派生窗口套接字并清理。 |
| `BUILD.html` | §4 补充说明 | 不动 | 保留 `-s/--socket` 的运行时文件说明，作为函数签名调整的依据。 |

### 4. `MonitorInfo` 缺少显示器 id，无法完成 `clients().monitor` 的名称解析

1. 问题描述
   根 `CORE.html` §4.3 要求 `ClientInfo.monitor` 由 monitor id 解析为显示器名，但同节的 `MonitorInfo` 只有 `name` / `focused` / `active_ws`，没有数字 id；`hyprctl clients` 的 `monitor` 字段是显示器索引，缺少映射源就无法实现文档承诺。
2. 解决方法
   - `MonitorInfo` 增加 `id: i64`，保留文档已有字段。
   - `clients()` 先取 clients 与 monitors，按 `client.monitor == monitor.id` 映射名称；无效时回退 focused 显示器，再回退首个显示器。
   - 测试 fixture 补充 `id` 字段，覆盖映射与回退。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.3 `daemon/hyprland.rs` | 改 | `MonitorInfo` 增加 `pub id: i64`，并注明 `clients().monitor` 的 id → name 解析。 |
| `PROVIDER.html` | §4.1 Hyprland IPC | 增 | 在 `clients -j` 消费行标注：`monitor` 为显示器 id，需与 `monitors -j` 的 `id` 对齐。 |

## 二、开发中发现文档没写明的细节

### 1. `RegisterStatusNotifierItem` 的错误分层没有具体映射

1. 问题描述
   `breadpan/PROVIDER.html` §4.1 只列出 `InvalidArgs` 与 `Failed` 两种线上错误，`CORE.html` §5.3.1 只说参数解析失败要拒绝；但没有说明“哪些失败归 InvalidArgs、哪些归 Failed”，也没有说明重复报到是否仍算成功。
2. 解决方法
   - 参数解析失败（非法服务名、非法对象路径、空串）→ `org.freedesktop.DBus.Error.InvalidArgs`。
   - 运行时已停机或其它内部失败 → `Failed`。
   - 重复报到幂等成功；只有首次插入才广播 `Added`。
   - 客户端不再收到“吞掉错误后的成功空应答”。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/PROVIDER.html` | §4.1 方法应答 / 错误应答 | 增 | 补一张映射表：解析失败 → InvalidArgs；停机 / 内部失败 → Failed；重复报到幂等成功。 |
| `breadpan/CORE.html` | §5.3.1 报到 | 不动 | 保留参数解析与 upsert 流程，错误映射在上表补充。 |

### 2. 通知命令失败后的状态迁移没有具体接口

1. 问题描述
   根 `CORE.html` §4.6 注记要求命令线程对 `Timeout` / `Stopped` 按 bnote 建议处置（重试或重建），但没有说明 daemon 如何把这一失败反馈给通知中心，也没有重建入口。
2. 解决方法
   - `Timeout`：记录日志，保留当前服务，UI 继续乐观更新，下一次快照校正。
   - `Stopped`：命令线程向 daemon 发送 `NotificationStatus::Unavailable`，随后退出；通知中心显示不可用态与刷新按钮。
   - 重建只由用户点击刷新触发 `Request::NotificationRetry`，不自动重试。
   - `activation_token` 失败只跳过令牌回发，`ActionInvoked` 仍照常发送。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.6 `daemon/notify.rs` | 增 | 明确命令线程失败路径：Stopped → NotificationStatus::Unavailable 并退出；Timeout → 记日志等待刷新；令牌失败不阻断动作回发。 |
| `USER.html` | §5.2.6 通知中心 | 不动 | 不可用态与刷新按钮已定义，保持现状。 |

### 3. 系统状态只写“订阅”，没有具体接口与刷新策略

1. 问题描述
   根 `CORE.html` §4.5 写 `start(tx)` 订阅 UPower / NetworkManager / BlueZ / MPRIS，但没有列明各服务的属性名、信号名、刷新节拍与失败回退。实现如果只按文档无法确定该监听哪些 PropertiesChanged，也无法判断启动时是否需要主动拉取一次。
2. 解决方法
   - 保留 daemon 内的周期采样作为基线（音量/亮度 5s，电池/网络/蓝牙 30s），逐项降级为 `None`。
   - 每项状态只在值变化时广播 `StateDelta`，避免重复推送。
   - 外部工具缺席时保留 `None`，UI 隐藏入口或灰置面板项。
   - 后续可在同一 `SystemState` 上叠加 PropertiesChanged 信号增量，不改变窗口契约。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.5 `daemon/system.rs` | 改 | 补充当前实现的采样节拍与降级规则；或列出要订阅的具体接口/属性，明确信号驱动与轮询兜底的关系。 |
| `PROVIDER.html` | §4.2 系统 DBus 服务 | 不动 | 保留服务与缺席行为表，具体属性名归 CORE。 |

### 4. 特殊工作区显示时“底层普通工作区”的取法未定义

1. 问题描述
   根 `CORE.html` §4.3 要求 `active_workspace_name` 在特殊工作区显示时返回底层普通工作区，但没有说明从 `monitors` 还是 `workspaces` 取值、多个普通工作区时如何选择。
2. 解决方法
   - 先取 `monitors -j` 的 focused 显示器与 `activeWorkspace.name`。
   - 名称不以 `special:` 开头时直接返回。
   - 以 `special:` 开头时，在该显示器的 `workspaces -j` 中取第一个非 `special:` 工作区；没有则返回 `None`。
   - 恢复窗口时以该值为目标工作区，失败则不移动。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.3 `daemon/hyprland.rs` | 增 | 明确特殊工作区回退算法：monitors.activeWorkspace 优先；为 special 时取同屏首个非 special 工作区；无则 None。 |
| `USER.html` | §5.2.7 最小化托盘 | 不动 | 恢复目标“该屏当前活动工作区”已定义，具体取法归 CORE。 |

### 5. dbusmenu 非法布局节点的错误语义未写

1. 问题描述
   `breadpan/PROVIDER.html` §4.1 只规定方法调用失败与 `InvalidArgs / Failed`，没有说明 `GetLayout` 返回的某个子节点结构非法时，是跳过该节点返回部分结果，还是整体失败。`CORE.html` §5.4.1 也未覆盖该分支。
2. 解决方法
   - `GetLayout` 的每个子节点按 `(i32, a{sv}, av)` 严格反序列化。
   - 任一节点失败立即返回 `TrayError::Protocol`，不返回部分 `MenuSnapshot`。
   - 调用方把该错误作为菜单读取失败处理，不渲染半截菜单。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/PROVIDER.html` | §4.1 方法应答 / 错误应答 | 增 | 注明 GetLayout 子节点结构非法 → `TrayError::Protocol`，不返回部分快照。 |
| `breadpan/USER.html` | §4.2 消息与错误 | 不动 | `Protocol` 已定义为属性缺失/类型不符的兜底错误，保持现状。 |

### 6. 通知计时器与“中心打开 / 同 id 替换”的交互未完全写死

1. 问题描述
   `USER.html` §5.2.5 写明了超时时长和中心打开时隐藏贴栏通知，但没有明确：同 id 替换是否重置计时并保留原位置；中心打开期间超时的贴栏记录是否从中心删除；`transient` 与非 `transient` 的差异落在哪一层。
2. 解决方法
   - 同 id 替换：贴栏栈原位替换、重置计时；中心记录移到最前（新消息时间排序）。
   - 超时只移除贴栏卡片；`transient` 额外发送 `NotificationClose { Expired }` 删除中心条目，非 `transient` 中心条目保留。
   - 中心打开时隐藏窗口但保留记录与计时；关闭后只恢复未过期的贴栏记录。
   - 常驻条目不创建计时器；`Some(0)` 恒常驻。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.5 通知弹窗 | 增 | 补充同 id 替换的重置/原位规则，以及中心打开期间超时只影响贴栏卡片、transient 才删中心条目。 |
| `CORE.html` | §4.13 `bar/modules/info.rs` | 不动 | 生命周期与计时已在该节注记中描述，避免重复定义。 |

## 三、其它技术上踩的坑

### 1. zbus 方法并发执行导致“首见序”按完成顺序错位

1. 问题描述
   zbus 默认把接口方法派发为独立任务；两个 `RegisterStatusNotifierItem` 即使按 A、B 到达，也可能 B 先完成 `GetNameOwner` / 路由探测并先入册。若在 `ItemTracker::upsert` 里才分配 `next_seq`，首见序会变成完成序。
2. 解决方法
   - 在 `register_item` 首次 `await` 之前用 `AtomicU64` 领取到达序号。
   - 新增 `upsert_at(entry, seq)`：`Inserted` 使用到达序号；`Refreshed` / `Renamed` 保留原 `seq`。
   - 启动反向发现的候选先按服务名排序，再顺序注册，固定启动首见序。
   - `all()` / `ordered_ids()` 统一按 `seq` 升序输出。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/CORE.html` | §4.4 `tracker.rs` | 增 | 增加 `upsert_at(entry, seq)` 与 `arrival_seq` 的说明：到达序号在首次 await 前领取。 |
| `breadpan/CORE.html` | §5.3.1 报到 | 增 | 在 `GetNameOwner` 之前插入“领取到达序号”步骤。 |

### 2. RwLock 读守卫跨 `if let` / `await` 造成写锁死等

1. 问题描述
   信号任务里写 `if let Some(id) = bus.tracker.read().unwrap().find(...) { ... }` 时，读守卫可能活到整个 `if let` 块结束；若块内再取写锁，或信号泵线程同步等待写锁，而同线程上的任务正持有读守卫，就会死锁。把 `NameOwnerChanged` 的重绑定改成同步写锁后，生命周期测试直接超时。
2. 解决方法
   - 所有“读后可能写”的路径先把结果取出：`let found = { bus.tracker.read().unwrap().find(...) };`，再按 `found` 分支。
   - `debounce` 中的存在性检查也先算 `present`，再投递事件。
   - `NameOwnerChanged` 的重绑定保留在运行时任务中异步执行，不在信号泵线程同步阻塞。
   - 删除/重键时先释放登记锁，再清理 `pending` / `pending_menu`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/CORE.html` | §5.3.2 NameOwnerChanged | 增 | 注明重绑定不得在信号泵线程同步阻塞；读守卫不得跨越后续写锁或 await。 |
| `breadpan/CORE.html` | §4.6.2 `bus.rs` | 不动 | 共享簿记类型已定义，锁使用纪律在上条补充。 |

### 3. Hyprland v2 事件载荷取错 token，且特殊工作区事件名写错

1. 问题描述
   `monitoraddedv2` / `monitorremovedv2` 的载荷是 `MONITORID,MONITORNAME,DESCRIPTION`；早期解析取第一个 token，拿到的是数字 id，热插拔时会 spawn/kill 名为 `"0"` 的窗口。另 `activespecial` / `activespecialv2` 被误写成 `activespecialworkspace`，特殊工作区切换不会触发重同步。
2. 解决方法
   - v2 显示器事件取第二个 token；v1 仍取第一个 token。
   - 事件名改为 `activespecial` / `activespecialv2`，并补 `moveworkspace` / `moveworkspacev2` / `renameworkspace` 的重同步。
   - 单元测试 fixture 改成真实载荷顺序，并新增窗口 / 最小化事件解析用例。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `PROVIDER.html` | §4.1 Hyprland IPC | 增 | 在事件表补充 v2 载荷顺序与 `activespecial*` / `moveworkspace*` / `renameworkspace` 事件名。 |
| `CORE.html` | §4.3 `HyprlandEvent` | 不动 | 事件枚举已覆盖，载荷细节归 PROVIDER。 |

### 4. layer-shell 窗口位置与全局 CSS 作用域

1. 问题描述
   贴栏通知最初按 Bottom 锚定，实际跑到了屏幕底部；文档要求“任务栏正下方”。同时 CSS 全局 `window { min-height: 30px; border-bottom: ... }` 会作用到通知窗口、dock 面板与触发条，导致多余底边和高度。
2. 解决方法
   - 通知窗口改锚定 `Edge::Top`，`margin Top = 30px`，水平按标题区域中心计算 `margin Left`。
   - dock 用两个独立 layer-shell 窗口：2px 触发条 + 动态面板；两者 `exclusive_zone = 0`、`keyboard_mode = None`。
   - 主任务栏窗口加 `bread-bar` 类，通知窗口加 `bread-notify`，dock 面板/触发条分别加 `bread-dock` / `bread-dock-trigger`；全局 `window` 规则改为 `window.bread-bar`。
   - GTK4 滑块填充节点是 `scale highlight`（单数），不是 `highlights`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.5 通知弹窗 | 增 | 明确贴栏弹窗锚定任务栏下缘（Top + 30px），而非屏幕底部。 |
| `CORE.html` | §4.13 / §4.14 | 不动 | 几何数值已定义，锚定实现细节由本条补充，避免重复。 |

### 5. GTK 回调的 `'static` 要求与文档中的 `&self` 结构不一致

1. 问题描述
   CORE 把 `WindowApp`、`InfoCenterWidget`、`DockWidget`、`TrayWidget` 等写成普通结构体字段与 `&self` 方法；但 GTK 信号闭包要求 `'static`，需要 `Rc<Self>` / `Rc<RefCell<_>>`，并且闭包捕获前要显式 clone。`Popover::popdown()` 还会同步触发 `closed`，若此时持有 `RefCell` 借用会重入 panic。
2. 解决方法
   - 窗口模块内部用 `Rc` 持有可变状态；需要回调的方法改成 `self: &Rc<Self>`。
   - 闭包捕获前先 `let widget = self.clone();`，避免移动后再借用。
   - 所有 `popdown` / 重建路径先收集目标、释放 `RefCell` 借用，再执行关闭。
   - 文档中的字段类型保留为语义示意，不强行按字面编译。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.9 / §4.13 / §4.14 / §4.16 | 增 | 加通用注记：窗口模块字段为语义示意，GTK 生命周期下以 `Rc` / `RefCell` / `Mutex` 实现；回调方法可按需使用 `self: &Rc<Self>`。 |
| `CORE.html` | §4.15 `popup.rs` | 不动 | 借用纪律已在文件注记中描述，保持现状。 |

### 6. clap `trailing_var_arg` 吞掉后台化参数

1. 问题描述
   `bread __daemon --socket X` 中 `--socket` 位于子命令之后，被 `trailing_var_arg = true` 收进 `cmd`；子进程的 `Args.socket` 仍是 `None`，后台 daemon 忽略自定义套接字。
2. 解决方法
   - 后台化命令改为 `bread --socket X __daemon`，让 clap 先解析全局选项，再进入子命令。
   - 保留 `__daemon` 内部的 socket 参数消费逻辑。
   - 手工验证 `-s` 路径下控制套接字与派生窗口套接字一致。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.1 `main.rs` | 增 | 注明后台化 spawn 的参数顺序：`--socket` 必须在 `__daemon` 之前。 |
| `BUILD.html` | §4 补充说明 | 不动 | `-s/--socket` 的用户说明不变。 |

### 7. 协议测试在桌面总线上的 `NameTaken` 竞争

1. 问题描述
   `bnote` 协议测试需要占用 `org.freedesktop.Notifications`。直接在桌面会话跑 `cargo test` 时，dunst 等守护已持有该名，测试全部以 `NameTaken` 失败；若强制抢名又会干扰用户会话。
2. 解决方法
   - 测试只在 `DBUS_SESSION_BUS_ADDRESS` 存在时运行。
   - 启动会话返回 `NameTaken` 时打印提示并跳过该用例，不 panic。
   - 推荐 `dbus-run-session -- cargo test` 提供隔离总线。
   - 用进程内静态锁串行化协议测试，避免多个测试互相抢名。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `bnote/BUILD.html` | §2 开发命令 | 增 | 注明普通 `cargo test` 在桌面总线被占用时会跳过协议用例；隔离全量运行必须用 `dbus-run-session -- cargo test`。 |
| `BUILD.html` | §2 开发命令 | 不动 | 根项目只保留库测试的隔离命令，细节归 bnote BUILD。 |

### 8. zbus 接口级信号匹配会收到厂商自定义信号

1. 问题描述
   只按接口名订阅 `org.kde.StatusNotifierItem` 或 `com.canonical.dbusmenu` 时，任何同接口的自定义信号都会进入处理闭包，被误记为 `Changed` / `MenuChanged`；`dbusmenu` 的 `ItemActivationRequested` 也会混入。
2. 解决方法
   - SNI 侧只接受 `NewStatus` / `NewIcon` / `NewTitle` / `NewToolTip` / `NewOverlayIcon` / `NewAttentionIcon`。
   - dbusmenu 侧只接受 `LayoutUpdated` / `ItemsPropertiesUpdated`。
   - 成员名从 `msg.header().member()` 读取；先把 header 绑定到局部变量，避免临时值生命周期错误。
   - 未命中的成员直接返回，不修改 `pending` / `pending_menu`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/CORE.html` | §4.6.1 / §5.4.2 | 增 | 列出允许的 SNI `New*` 与 dbusmenu 成员白名单，明确其它信号忽略。 |
| `breadpan/USER.html` | §4.3 | 不动 | 厂商扩展不保证透传的边界已定义。 |

### 9. GTK 主题投影被当成菜单行的下边框

1. 问题描述
   dbusmenu 自绘菜单的每一行都是透明 `GtkButton`，紧密堆叠后看起来每条下面都挂着一条细线。通用 `button` 规则只重置了 `background` / `border` / `border-radius`，没有重置主题设置的 `box-shadow`；Adwaita（`0 1px 2px`）与 Breeze（`0 1px 1px`）都会给按钮加投影，落在 `fg` 浅色菜单底上就被读成了行下边框。
2. 解决方法
   - 通用 `button` 规则补 `box-shadow: none;`，让透明底按钮同时不投影；菜单行不再单独覆盖，避免换主题后从别的按钮漏出同类问题。
   - 保留 `.menu-popover contents` 的投影，弹窗面板与任务栏的分离仍由它承担。
   - 菜单分隔仍只由 dbusmenu 的 `Separator` 节点绘制（`gold` 1px，上下 3px / 水平 6px），行与行之间不画线。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.1 任务栏窗口与按钮 | 改 | 按钮外观补“无投影”：透明底、无边框、无投影、圆角 6px、padding 0，明确主题投影也须清零。 |
| `USER.html` | §5.2.3 托盘与托盘菜单 | 不动 | “分隔线 1px”已限定分隔只来自菜单项，行间不加边框的语义可由按钮外观条款推出。 |
