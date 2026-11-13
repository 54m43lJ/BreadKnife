# IMPL — 开发过程记录

> 基于 `dev/evol-bnote` 分支、文档基线 `ee16a0d` 的实现过程整理。
> 每条只记录一个主题；同一事实不在不同大块重复。

## 一、开发中发现文档的错误

### 1. bnote `report_fatal` 的同步签名与调用栈互相矛盾 [已解决]

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

4. 实际操作
   - 采用“坚持同步签名”的备选方案：`bnote/CORE.html` §4.5 保留 `pub(crate) fn report_fatal(&self, message: String);`，在签名注记中补充“内部派发运行时任务，按 §5.8 的 `send_fatal(...).await → claim_stop() → events.close()` 顺序执行，调用方不等待”。
   - §5.8 未改动，继续作为终止序列的权威描述。

### 2. breadpan 报到事件顺序与“同一 Item 事件严格有序”冲突 [已解决]

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

4. 实际操作
   - `breadpan/CORE.html` §5.3.1 调用栈改为 `upsert → events.send(Added(id)) → iface::emit_registered(service)`，并注明 `emit_registered` 由独立运行时任务执行、广播不得阻塞 `Added`。
   - `breadpan/USER.html` §4.2 未改动，继续以“同一 Item 事件严格有序”作为契约依据。

### 3. 根 `shutdown_files()` 没有覆盖自定义控制套接字 [已解决]

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

4. 实际操作
   - 采用与 `src/shared/protocol.rs` / `Daemon::shutdown_files` 现有实现一致的方案，未把 `protocol::shutdown_files` 改成接收 `control: &Path`：根 `CORE.html` §4.18 保留无参签名，并在代码注释与注记中补充“仅清理默认 PID / 控制 / 派生窗口套接字；自定义路径由 `Daemon::shutdown_files()` 追加清理”。
   - 根 `CORE.html` §5.3 Exit 调用栈由直接调用 `protocol::shutdown_files()` 改为 `Daemon::shutdown_files()`，并展开为“默认文件清理 + 按跟踪的 `control_path` / `window_path` 清理自定义控制套接字及派生窗口套接字”。
   - `BUILD.html` §4 未改动，`-s/--socket` 的运行时文件说明继续作为依据。

### 4. `MonitorInfo` 缺少显示器 id，无法完成 `clients().monitor` 的名称解析 [已解决]

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

4. 实际操作
   - `CORE.html` §4.3 的 `MonitorInfo` 展开为多行结构并增加 `pub id: i64`，注明 `monitors -j` 的 `id` 与 `clients -j` 的 `monitor` 对齐；`ClientInfo.monitor` 注释补充“`RawClient.monitor`（显示器 id）对齐 `MonitorInfo.id` 后解析为显示器名”。
   - `CORE.html` §4.3 注记补充 `clients()` 的 id → name 解析与回退链：按 `MonitorInfo.id` 匹配，无效时回退当前聚焦显示器。
   - `PROVIDER.html` §4.1 的 `hyprctl clients -j` 消费行标注 `monitor` 为显示器 id，需与 `monitors -j` 的 `id` 对齐。
   - 仅改文档；`src/daemon/hyprland.rs` 现有的 `MonitorInfo.id`、`clients()` 映射与回退实现未改动。

## 二、开发中发现文档没写明的细节

### 1. `RegisterStatusNotifierItem` 的错误分层没有具体映射 [已解决]

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

4. 实际操作
   - `breadpan/PROVIDER.html` §4.1 在“错误应答”后新增“报到结果映射”表：参数解析失败 → `InvalidArgs` 且不写登记簿；运行时已停机或其它内部失败 → `Failed`；首次报到与同名 / 同一对象重复报到均为成功空应答，重复报到幂等，只有首次插入且快照栅栏已置位时才广播 `StatusNotifierItemRegistered` 并入队 `Added`。
   - `breadpan/CORE.html` §5.3.1 未改动，继续保留参数解析与 `upsert` 流程。
   - 仅改文档；错误映射与 `breadpan/src/dbus/iface.rs`、幂等入册与首次插入广播逻辑与 `breadpan/src/dbus/mod.rs` 现有实现一致。

### 2. 通知命令失败后的状态迁移没有具体接口 [已解决]

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

4. 实际操作
   - `CORE.html` §4.6 把原“命令线程失败记录日志”注记改为分层描述：`Stopped` 上报 `NotificationStatus::Unavailable { message }` 并退出命令线程，重建只由刷新按钮的 `Request::NotificationRetry` 触发；`Timeout` / 其它错误只记日志、保留会话，UI 乐观更新并由下一次快照校正；两种情况都不自动重试。
   - `CORE.html` §4.6 补充 `invoke_action` 的令牌失败语义：非 `Stopped` 只跳过令牌回发，`ActionInvoked` 仍照常发送；`Stopped` 按命令线程停止路径处理。
   - `USER.html` §5.2.6 未改动，不可用态与刷新按钮继续作为窗口侧契约。

### 3. 系统状态只写“订阅”，没有具体接口与刷新策略 [已解决]

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

4. 实际操作
   - `CORE.html` §4.5 新增注记，写明当前采样基线：私有 tokio 运行时周期轮询，音量 / 亮度 `FAST_POLL = 5s`，电池 / 网络 / 蓝牙 `SLOW_POLL = 30s`；当前不订阅 `PropertiesChanged`，轮询是唯一数据源，后续信号增量只作为叠加且保留轮询兜底。
   - `CORE.html` §4.5 列明各服务消费口径：UPower `DisplayDevice` 的 `Percentage` / `State`；NetworkManager `PrimaryConnectionType` 及类型映射；BlueZ `GetManagedObjects` 的 `Adapter1.Powered` / `Device1.Connected`；MPRIS 只在 `media_control` 请求时经 session bus 访问 Player，不参与周期采样；音量后端 `pamixer → wpctl → pactl`，亮度 `brightnessctl -c backlight` → `ddcutil`。
   - `CORE.html` §4.5 补充降级规则：总线 / 服务 / 工具缺席时对应字段保持 `None`，不阻塞其它项；仅在值变化时发送 `DaemonEvent::System`，daemon 再对整份状态去重后广播 `StateDelta.system`。
   - `PROVIDER.html` §4.2 未改动，继续保留服务清单与缺席行为表。

### 4. 特殊工作区显示时“底层普通工作区”的取法未定义 [已解决]

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

4. 实际操作
   - `CORE.html` §4.3 的 `active_workspace_name` 签名行补充注释：优先取 `monitors.active_ws`；为 `special:` 时取同屏首个非 `special` 工作区，无则 `None`。
   - `CORE.html` §4.3 注记展开完整回退算法：`monitors -j` 的 `active_ws.name` 优先；非 `special:` 直接返回；`special:` 时在 `workspaces -j` 中按该显示器找第一个非 `special:` 工作区；没有则 `None`。恢复窗口以该值为目标，移动失败则不移动。
   - `USER.html` §5.2.7 未改动，恢复目标仍由窗口契约定义为“该屏当前活动工作区”。

### 5. dbusmenu 非法布局节点的错误语义未写 [已解决]

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

4. 实际操作
   - `breadpan/PROVIDER.html` §4.1 新增“菜单读取应答”注记：库内菜单读取对 `GetLayout` 的每个子节点按 `(i32, a{sv}, av)` 严格反序列化，任一子节点结构非法立即返回 `TrayError::Protocol`，不跳过节点、不返回部分 `MenuSnapshot`。
   - `breadpan/PROVIDER.html` §4.1 补充调用方语义：把该错误作为整层菜单读取失败处理，不渲染半截菜单；`TrayError::Protocol` 的窗口侧含义仍由 `USER.html` §4.2 定义。
   - `breadpan/USER.html` §4.2 未改动。

### 6. 通知计时器与“中心打开 / 同 id 替换”的交互未完全写死 [已解决]

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

4. 实际操作
   - `USER.html` §5.2.5 的堆叠规则补充同 id 替换语义：贴栏栈原位更新并重置该条计时，不移动到栈顶；通知中心记录移到最前，按新消息时间排序。
   - `USER.html` §5.2.5 的中心打开规则改为明确超时作用层：隐藏期间计时照常，超时只移除贴栏卡片；非 `transient` 条目保留在通知中心等待清理，`transient` 条目同时删除中心记录；关闭中心后只恢复仍存活且未过期的贴栏卡片。
   - `CORE.html` §4.13 未改动，生命周期与计时的实现注记保持现状。

## 三、其它技术上踩的坑

### 1. zbus 方法并发执行导致“首见序”按完成顺序错位 [已解决]

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

4. 实际操作
   - `breadpan/CORE.html` §4.4 在 `ItemTracker` 增加 `upsert_at(entry, seq)` 签名，注明 `Inserted` 使用传入序号、`Refreshed` / `Renamed` 保留原值。
   - `breadpan/CORE.html` §4.4 注记补充 `BusCenter.arrival_seq`（`AtomicU64`）：报到在首次 `await` 前领取到达序号，再交给 `upsert_at`；并发完成顺序不再影响首见序；启动反向发现按服务名排序后顺序注册。
   - `breadpan/CORE.html` §5.3.1 调用栈在 `split_service_arg` 与 `GetNameOwner` 之间插入 `bus.arrival_seq.fetch_add(1)`，并把入册调用改为 `upsert_at(ItemEntry, seq)`；注记补充“到达序号必须在首次 await 前领取”。

### 2. RwLock 读守卫跨 `if let` / `await` 造成写锁死等 [已解决]

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

4. 实际操作
   - `breadpan/CORE.html` §5.3.2 新增注记：重绑定与死亡清理必须在运行时任务中异步执行，信号泵线程不得同步等待 `tracker` 写锁，避免同线程读守卫与写锁互相死锁。
   - `breadpan/CORE.html` §5.3.2 补充锁使用纪律：读守卫不得跨越后续写锁或 `await`，读后可能写的路径先用显式块取出结果再分支；删除 / 重键先释放登记锁，再清理 `pending` / `pending_menu`。
   - `breadpan/CORE.html` §4.6.2 未改动，共享簿记类型保持现状。

### 3. Hyprland v2 事件载荷取错 token，且特殊工作区事件名写错 [已解决]

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

4. 实际操作
   - `PROVIDER.html` §4.1 事件表拆分显示器事件：v1 `monitoradded` / `monitorremoved` 取第一个 token；v2 `monitoraddedv2` / `monitorremovedv2` 载荷为 `MONITORID,MONITORNAME,DESCRIPTION`，取第二个 token 作为显示器名。
   - `PROVIDER.html` §4.1 增补 `activespecial` / `activespecialv2`（特殊工作区切换）与 `moveworkspace` / `moveworkspacev2` / `renameworkspace`（工作区移动 / 重命名）均触发全量重同步。
   - `CORE.html` §4.3 未改动，`HyprlandEvent` 枚举保持现状，载荷细节由 PROVIDER 承载。

### 4. layer-shell 窗口位置与全局 CSS 作用域 [已解决]

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

4. 实际操作
   - `USER.html` §5.2.5 新增定位注记：贴栏弹窗锚定任务栏下缘而非屏幕底部；层壳使用 Top 边，top margin 相对任务栏让出的可用区。
   - 间距按当前实现 `POPUP_TOP_GAP = 5px` 写入，未照抄当时修订建议中的 30px；30px 是早期临时值，后续已调整为 5px，文档按现值描述避免回退。
   - `CORE.html` §4.13 / §4.14 未改动，几何与窗口分类仍由 CORE 承载。

### 5. GTK 回调的 `'static` 要求与文档中的 `&self` 结构不一致 [已解决]

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

4. 实际操作
   - `CORE.html` §4.9 / §4.13 / §4.14 / §4.16 各新增同一条通用注记：字段类型为语义示意，GTK 生命周期下可变状态以 `Rc` / `RefCell` / `Mutex` 持有；需要被信号闭包捕获的回调方法可使用 `self: &Rc<Self>`，捕获前先 clone 句柄。
   - `CORE.html` §4.15 未改动，`popdown` / `RefCell` 借用纪律继续由该节现有注记承载。

### 6. clap `trailing_var_arg` 吞掉后台化参数 [已解决]

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

4. 实际操作
   - `CORE.html` §4.1 新增注记：后台化 spawn 固定使用 `bread --socket X __daemon`，`--socket` 必须在 `__daemon` 之前，先由 clap 解析为全局 `Args.socket`。
   - 注记同时说明原因：`cmd` 使用 `trailing_var_arg = true`，放在子命令之后的选项会被收进 `cmd`，后台 daemon 会退回默认套接字。
   - `BUILD.html` §4 未改动，`-s/--socket` 的用户说明保持现状。

### 7. 协议测试在桌面总线上的 `NameTaken` 竞争 [已解决]

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

4. 实际操作
   - `bnote/BUILD.html` §2 的 `cargo test` 行补充“桌面总线被占用时跳过”，并在命令块后新增注记：普通 `cargo test` 遇到 `org.freedesktop.Notifications` 被 dunst 等占用时打印跳过提示并以 `NameTaken` 跳过，不 panic、不抢名。
   - `bnote/BUILD.html` §2 注明隔离全量运行必须使用 `dbus-run-session -- cargo test`；未设置 `DBUS_SESSION_BUS_ADDRESS` 时协议用例同样跳过；协议用例用进程内静态锁串行化。
   - `BUILD.html` §2 未改动，根项目继续只列出库测试的隔离命令。

### 8. zbus 接口级信号匹配会收到厂商自定义信号 [已解决]

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

4. 实际操作
   - `breadpan/CORE.html` §4.6.1 新增 SNI 信号白名单注记：只接受 `NewStatus` / `NewIcon` / `NewTitle` / `NewToolTip` / `NewOverlayIcon` / `NewAttentionIcon`，同接口厂商自定义信号忽略。
   - `breadpan/CORE.html` §4.6.1 补充成员名读取方式：从 `msg.header().member()` 读取，先把 header 绑定到局部变量；未命中的成员直接返回，不修改 `pending`。
   - `breadpan/CORE.html` §5.4.2 新增 dbusmenu 白名单：只接受 `LayoutUpdated` / `ItemsPropertiesUpdated`；`ItemActivationRequested` 等其它同接口成员忽略，不修改 `pending_menu`。
   - `breadpan/USER.html` §4.3 未改动。

### 9. GTK 主题投影被当成菜单行的下边框 [已解决]

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

4. 实际操作
   - `USER.html` §5.2.1 的按钮外观补充“无投影”：透明底、无边框、无投影、圆角 6px、padding 0，并注明主题默认的 `box-shadow` 也须清零。
   - `USER.html` §5.2.3 未改动；菜单行不画线仍由分隔只来自 `Separator` 节点的既有条款表达。
   - 仅改文档；`src/assets/bread.css` 的通用 `button` 规则已包含 `box-shadow: none`，未改代码。

### 10. 工作区条整条重建，快速连点吞掉点击 [已解决]

1. 问题描述
   工作区条每次状态推送都清空容器并重建全部 `GtkButton`。快速连点不同工作区时，前一次点击触发的 Resync 推送会在后一次点击的按下与松开之间销毁按钮，松开事件落到新按钮上，`clicked` 不再配对，后续点击就被吞掉；同时按压缩放态与悬停浮层也随之丢失。
2. 解决方法
   - `buttons` 的值从 `gtk::Button` 改为 `WsButton`，同时保存基底、叠加点与浮层数据的句柄。
   - `rebuild` 改为按 id 增量对账：删除消失的工作区、复用仍存在的按钮并原地更新样式类/可见性与浮层数据，只新建新增项。
   - 仅在工作区集合或顺序变化时重排；活跃态、紧急态的推送不再移动按钮，避免打断进行中的按下。
   - 点击回调从浮层数据单元格读取当前工作区名，不再固化创建时的名字。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.12 `bar/modules/workspace.rs` | 改 | `buttons` 改为 `RefCell<BTreeMap<i64, WsButton>>`；`rebuild` 语义改为“按 id 增量对账（复用/更新/增删），集合与顺序未变时不重排”，并注明整条重建会在按下/松开之间销毁按钮而丢失 `clicked`。 |
| `USER.html` | §5.2.2 Workspace 按钮 | 增 | 补一条可验收行为：快速连续点击不同工作区时每次点击都应生效，不因状态推送吞点击。 |

4. 实际操作
   - `CORE.html` §4.12 的 `buttons` 改为 `Rc<RefCell<BTreeMap<i64, WsButton>>>`，并补充 `WsButton` / `WsTip`：按钮、基底、叠加点与浮层数据都保留可原地更新的句柄。
   - `CORE.html` §4.12 注记明确 `rebuild` 按 id 增量对账：删除消失项、复用并原地更新仍存在的按钮、只新建新增项；仅集合或顺序变化时重排，活跃 / 紧急推送不移动按钮；整条重建会在按下与松开之间销毁按钮并丢失 `clicked`。
   - `CORE.html` §4.12 注记补充点击回调从浮层数据单元格读取当前工作区名，不固化创建时的名字。
   - `USER.html` §5.2.2 增加可验收行为：快速连续点击不同工作区时每次点击都应生效，状态推送不得在按下与松开之间重建按钮而吞掉 `clicked`。

### 11. 重同步去抖窗口 200ms 偏高，工作区切换更新有滞后 [已解决]

1. 问题描述
   工作区状态本来就由 `.socket2.sock` 事件驱动，并非轮询；但工作区事件被折叠为 `Resync` 后，要等 `recv_timeout(TICK)` 超时才处理，`TICK = 200ms` 让切换后的高亮最多晚一个滴答，体感像轮询。核心问题只是去抖窗口过长，不需要拆分事件类型。
2. 解决方法
   - `TICK` 从 200ms 降到 50ms，工作区与窗口生命周期事件仍在同一去抖窗口内合并，但更新延迟压到一个帧量级。
   - 保持原有事件面与 `pending_resync` 机制不变：`recv_timeout` 是等待事件，超时档只处理待重同步，不引入周期轮询。
   - 撤回“工作区事件单独直达”的改动，避免为 50ms 级的延迟引入额外事件分支。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.2 `daemon/mod.rs` | 改 | `run_loop` 注释补 `TICK = 50ms`：滴答仅作重同步去抖窗口，`recv_timeout` 是阻塞等待事件而非轮询。 |
| `CORE.html` | §5.1.1 daemon 启动 | 不动 | 调用栈的 `rx.recv_timeout(TICK)` 与“超时且 `pending_resync` → `resync()`”保持现状，仅数值变化。 |

4. 实际操作
   - `CORE.html` §4.2 的 `run_loop` 注释补上 `TICK = 50ms`，并注明该滴答仅作重同步去抖窗口，`recv_timeout` 是阻塞等待事件而非周期轮询。
   - `CORE.html` §5.1.1 未改动，`rx.recv_timeout(TICK)` 与“超时且 `pending_resync` → `resync()`”保持现状。
   - 仅改文档；`src/daemon/mod.rs` 的 `TICK` 已是 50ms，未改代码。

### 12. tooltip 配色与工作区浮层共用一套容器规则 [已解决]

1. 问题描述
   `tooltip` 规则同时承担普通 tooltip 与工作区悬停浮层的外观：浮层通过 `set_custom` 塞进 tooltip 容器，配色只能跟着 tooltip 走，内容再套样式会出现双重内边距/描边的风险；而浮层语义上并不是 tooltip，需要独立定义。
2. 解决方法
   - `tooltip` 容器与内部 `box` 设为透明、无内边距；普通 tooltip 的 `surface` 底、`fg` 字、`gold` 1px 边框、圆角 8px、内边距 6px／水平 8px 落在 `tooltip > box > label`。
   - `.ws-tip` 独立定义完整外观：`surface` 底、`fg` 字、`gold` 1px 边框、圆角 8px、内边距 6px／水平 8px，并保留标题 / 窗口行 / 空态的配色。
   - 两处使用同一套色板（`surface` / `fg` / `gold`），视觉统一但不共享选择器。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.2 Workspace 按钮 | 不动 | 浮层配色（`surface` / `fg` / `gold` / 圆角 8px）已在现有条款中，样式实现分离不改变契约。 |
| `USER.html` | §2 通用约定 | 不动 | 按要求不新增“浮层不属于 tooltip”的规则，只保留 `bread.css` 中的样式定义。 |

4. 实际操作
   - 两项文档动作均为“不动”，因此 `USER.html` §5.2.2 与 §2 未改动：浮层配色（`surface` / `fg` / `gold` / 圆角 8px）已在 §5.2.2 现有条款中，本次样式实现分离不改变窗口契约。
   - `src/assets/bread.css` 已按实现分离：`tooltip` 容器与内部 `box` 透明、无内边距，普通 tooltip 外观落在 `tooltip > box > label`；`.ws-tip` 独立定义完整外观，两者共用同一套色板但不共享选择器。
   - 未改代码；本次仅将该项标记为已解决并记录实际处理方式。

### 13. 工作区基底与紧急/活跃对比色改取 fg / purple [已解决]

1. 问题描述
   原实现的基底色为 `purple`、紧急态取 `fg`，活跃叠点取 `fg`；用户要求基底改 `fg`，紧急与活跃的对比色统一用 `purple`，紧急态仍保持两色互换。`cream` 在深色任务栏上过亮，不作为最终基色。
2. 解决方法
   - 常态基底（圆点与长方形）改为 `fg`；活跃叠点改 `purple`。
   - 紧急基底改 `purple`；紧急基底上的活跃叠点改 `fg`，保持两色互换。
   - 过渡仍为 150ms ease，形态与尺寸不变。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.2 Workspace 按钮 | 改 | 基底常态 `fg`、活跃叠点 `purple`；紧急基底 `purple`、叠点 `fg`。 |
| `ABOUT.html` | 「界面交互」工作区矩阵 note | 改 | 背景色改 `fg`、对比色改 `purple`；紧急态仍为两色互换。 |

4. 实际操作
   - `USER.html` §5.2.2 更新配色：常态基底（圆点与长方形）取 `fg`，活跃叠点取 `purple`；紧急基底取 `purple`，紧急基底上的活跃叠点取 `fg`；无窗口且活跃的视觉改为单个 `purple` 圆点。
   - `ABOUT.html`「界面交互」工作区矩阵注记更新为背景色 `fg`、前景色（对比色）`purple`，紧急态两色互换（长方形 `purple`、叠点 `fg`）。
   - 同步更新 `style.css` 的工作区示意控件颜色，使文档图示与文字契约一致；过渡仍为 150ms ease，形态与尺寸未变。

### 14. 托盘背景块与 hover 配色 [已解决]

1. 问题描述
   `cream` 作托盘底色在任务栏上过亮；改用 `fg` 后，普通 hover 若直接落到通用 `surface` 会破坏底色层次，需要一个半透明叠加。背景块最初只留 2px 上下 margin，顶部空间过紧；对称加大后又显得几何居中、视觉重心偏高。
2. 解决方法
   - `.tray-box` 与 `.tray-btn` 背景改 `fg`，`.tray-box` 采用非对称 `margin: 5px 0 3px 0`，视觉重心略低于几何中心。
   - 保持默认 `valign = Fill`：非对称 margin 下可稳定得到上 5 / 下 3，`Center` 会把剩余空间上下均分。
   - `.tray-btn:hover` 使用 `alpha(@muted, 0.35)` 叠在 `fg` 基底上；GTK CSS 的 `alpha()` 可用，无需回退 `purple`。
   - 移除 `tray-attention` 的独立 hover 覆写，所有托盘项统一走半透明 muted；NeedsAttention 的额外视觉不在本轮范围。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.3 托盘与托盘菜单 | 改 | 托盘区域与图标按钮底色改 `fg`；hover 用 35% `muted` 叠加；背景块上 5px、下 3px，保持默认填充对齐。 |

4. 实际操作
   - `USER.html` §5.2.3 明确托盘区域与图标按钮底色为 `fg`，并补充背景块采用上 5px、下 3px 的非对称外边距，保持默认填充对齐（`valign = Fill`），视觉重心略低于几何中心。
   - `USER.html` §5.2.3 的图标按钮条款补充 Hover：用 35% `muted` 半透明叠加在 `fg` 基底上；失联图标仍按 35% 不透明度置灰。
   - `src/assets/bread.css` 已包含 `.tray-box` / `.tray-btn` 的 `fg` 底色、非对称 margin 与 `alpha(@muted, 0.35)` hover，未改代码。

### 15. 注册期 Menu 探测失败导致 blueman 右键落到 ContextMenu [已解决]

1. 问题描述
   blueman 的 SNI 对象实现了 `Menu` 属性与 `com.canonical.dbusmenu`，但不实现 `ContextMenu` 方法。注册期 `probe_routing` 若瞬时失败或早于属性就绪，登记条目 `menu = None`；右键按路由表落到 `ContextMenu`，两个前缀都失败后报 “No such method ContextMenu / No such interface org.freedesktop.StatusNotifierItem”。
2. 解决方法
   - 新增 `ensure_menu`：交互时若登记条目无菜单，补探一次 `Menu` / `ItemIsMenu`，成功即回写登记簿并继续 Menu 流程。
   - 右键先补探再判定，仍无菜单才走 `ContextMenu` fallback。
   - 左键在 `menu_only` 与 Activate 失败回退两处同样补探，避免注册竞态让整项即菜单的 Item 失去菜单。
   - 生命周期测试让 mock 的 `Menu` 属性在三次注册期间不可读、右键前才就绪，断言右键仍返回 `InteractionOutcome::Menu`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `breadpan/CORE.html` | §4.6.7 `interact.rs` / §5.5.2 | 改 | 增加 `ensure_menu` 签名与流程：无菜单时交互期补探 `Menu`，成功回写登记簿；右键在补探后仍无菜单才 `ContextMenu`。 |
| `breadpan/USER.html` | §3.6 交互方法 | 改 | `right_click` 注释补“注册期探测缺失时交互时补探一次”；`TrayItem.menu` 说明注明该值为现场读取，注册期探测失败不永久生效。 |

4. 实际操作
   - `breadpan/CORE.html` §4.6.7 增加 `ensure_menu(bus, entry) -> bool` 签名：无菜单时重探 `Menu` / `ItemIsMenu`，成功回写登记簿并更新条目。
   - `breadpan/CORE.html` §5.5.2 调用栈改为右键先 `ensure_menu`，有菜单走 `menu::read_layer`，补探后仍无菜单才 `ContextMenu`；注记补充左键的 `menu_only` 与 Activate 失败回退同样先补探。
   - `breadpan/USER.html` §3.6 的 `right_click` 注释补充“注册期探测缺失时交互期补探一次，仍无菜单才 ContextMenu fallback”；§3.5 与 `TrayItem.menu` 的字段说明注明该值为现场读取，注册期探测失败不永久生效，交互时会补探。
   - 仅改文档；`breadpan/src/dbus/interact.rs` 现有 `ensure_menu` 与补探逻辑未改动。

### 16. 音量只认 pamixer、亮度误读键盘 LED，导致拿不到数据 [已解决]

1. 问题描述
   环境没有 `pamixer`，音量读取直接返回 `None`；亮度读取执行 `brightnessctl -m` 时取第一行（可能是键盘 LED 设备）并解析最后一个字段（max），成功返回了错误值，`ddcutil` 外接显示器回退因此永远不会执行。表现为音量和亮度都没有可用数据。
2. 解决方法
   - 音量读取改为后端链 `pamixer → wpctl → pactl`；分别解析 `--get-volume/--get-mute`、`get-volume`（含 `[MUTED]`）、`get-sink-volume/get-sink-mute`。
   - 音量设置使用同一后端顺序，命中即止，避免重复设置。
   - 亮度读取改用 `brightnessctl -m -c backlight`，从 `device,class,current,percent%,max` 取百分比字段；无内置面板时才回退 `ddcutil getvcp 10 --brief`。
   - 亮度设置同时尝试 `brightnessctl` 与 `ddcutil`，两者都失败才打印错误。
   - 为 wpctl / pactl / brightnessctl / ddcutil 的输出格式补纯函数解析测试。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `PROVIDER.html` | §4.3 外部工具 | 改 | 音量增加 `wpctl` / `pactl` 回退；`brightnessctl` 注明只读 `backlight` 类并取百分比字段。 |
| `BUILD.html` | §1 依赖表“可选系统工具” | 改 | 补充 `wpctl`（PipeWire）/ `pactl`（PulseAudio）为音量后端回退。 |
| `CORE.html` | §4.5 `daemon/system.rs` | 改 | `set_volume` 注释改为后端链；补充亮度读取的后端顺序与 `backlight` 过滤语义。 |

4. 实际操作
   - `PROVIDER.html` §4.3 外部工具表把音量拆为 `pamixer` / `wpctl` / `pactl` 三行，注明按 `pamixer → wpctl → pactl` 回退，全部缺席才灰置音量滑块；`brightnessctl` 行注明读取只用 `-c backlight` 并取百分比字段，`ddcutil` 行注明无内置面板时的读取回退。
   - `PROVIDER.html` §4.3 注记补充音量读取 / 设置的后端顺序 `pamixer → wpctl → pactl`、亮度读取的 `backlight` 过滤，以及亮度设置同时尝试 `brightnessctl` 与 `ddcutil`、两者都失败才报错。
   - `BUILD.html` §1 可选系统工具补充 `wpctl`（PipeWire）/ `pactl`（PulseAudio）作为音量后端回退。
   - `CORE.html` §4.5 更新 `set_volume` / `set_brightness` 注释，并把音量与亮度后端链、`backlight` 过滤语义写入注记。
   - 仅改文档；`src/daemon/system.rs` 现有后端链与解析实现未改动。

### 17. 贴栏通知顶部空白过大 [已解决]

1. 问题描述
   `NotificationWindow` 用层壳 `Edge::Top` margin 30 定位贴栏通知；层壳 margin 相对任务栏让出的可用区，而不是屏幕顶边，因此实际形成“任务栏 31px + margin 30px”的 30px 空白，用户只想要约 5px。
2. 解决方法
   - 新增常量 `POPUP_TOP_GAP = 5`，`set_margin(Edge::Top, POPUP_TOP_GAP)`。
   - 实测层壳几何：任务栏 h=31、通知窗 y=36，间距正好 5px。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.5 通知弹窗 | 改 | 明确“任务栏底边到通知窗顶部 5px”，并注明层壳 top margin 取 5（相对可用区）。 |
| `CORE.html` | §4.13 `bar/modules/info.rs` | 改 | 记录 `POPUP_TOP_GAP` 常量及层壳 margin 语义，避免再次按屏幕顶边推算。 |

4. 实际操作
   - `USER.html` §5.2.5 的定位条款改为明确“任务栏底边到通知窗顶部为 5px”，层壳 Top margin 取 `POPUP_TOP_GAP = 5`，相对任务栏让出的可用区而非屏幕顶边。
   - `CORE.html` §4.13 增加 `POPUP_TOP_GAP` 常量（推荐值 5），并在注记中说明 `set_margin(Edge::Top, POPUP_TOP_GAP)` 的可用区语义，附实测任务栏 h=31 / 通知窗 y=36，避免再按屏幕顶边累加。
   - 仅改文档；`src/bar/modules/info.rs` 已使用 `POPUP_TOP_GAP = 5`，未改代码。

### 18. 通知中心组标题箭头不随收缩状态变化、面板高度不回落 [已解决]

1. 问题描述
   组标题箭头用 `expanded`（“展开全部”状态）决定方向，普通预览态的展开/收缩不会改变箭头；组收起后内容虽被移除，但 popover 在可见状态下保持旧高度，不会自动回落。仅 queue resize 不足以更新已可见的 xdg_popup，必须重新 present。
2. 解决方法
   - 箭头方向改由 `!collapsed.contains(app)` 决定：收起 `▸`、展开 `▾`；`expanded` 仅用于“展开全部”隐藏其它组。
   - `CenterView` 保存 popover、滚动区与最大高度；每次重建后测量内容自然高度，`set_min_content_height(min(natural, max_height))`，再对可见 popover 调用 `present()` 重新呈现。
   - 独立 GTK 探针验证：内容从 8 项减到 2 项后，popover 高度由 204 收缩到 72。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.6 通知中心 | 改 | 明确组标题箭头与展开/收缩一致（收起 `▸`、展开 `▾`），面板高度随组状态实时变化。 |
| `CORE.html` | §4.13 `bar/modules/info.rs` | 改 | `CenterView` 保存 popover / scroll / max_height；`rebuild` 末尾以 `fit_height()` 同步滚动区与弹窗高度。 |

4. 实际操作
   - `USER.html` §5.2.6 的聚合标题条款补充：组标题箭头与展开/收缩一致（收起 `▸`、展开 `▾`），面板高度随组状态实时变化。
   - `CORE.html` §4.13 增加 `CenterView` 结构（保存 `content` / `popover` / `scroll` / `max_height`）与 `rebuild` / `fit_height` 方法签名。
   - `CORE.html` §4.13 注记明确箭头方向由 `!collapsed.contains(app)` 决定，`expanded` 只用于“展开全部”；`fit_height()` 测量自然高度、设置滚动区最小高度并对可见 popover 重新 `present()`，避免已可见 xdg_popup 保持旧高度。
   - 仅改文档；`src/bar/modules/info.rs` 现有箭头与 `fit_height` 实现未改动。

### 19. 堆叠通知之间多出一条分隔线 [已解决]

1. 问题描述
   `NotificationWindow::rebuild` 在每两张通知卡之间插入一个 `GtkSeparator`（`.notif-sep`，`gold` 1px），用户认为堆叠弹窗之间不应画线。
2. 解决方法
   - 删除重建循环中的分隔线节点。
   - 删除 `.notif-sep` 样式；卡片之间只保留 `.notif-card` 的 4px 下边距。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.5 通知弹窗 | 改 | 删除“条间分隔线 1px”，改为“多条之间以 4px 间距分隔，不画线”。 |

4. 实际操作
   - `USER.html` §5.2.5 删除“条间分隔线 1px”，改为“多条之间以 4px 间距分隔，不画线”。
   - 仅改文档；`src/bar/modules/info.rs` 已不再插入 `GtkSeparator`，`src/assets/bread.css` 的 `.notif-card` 已使用 `margin: 0 0 4px 0`，未改代码。

### 20. dismiss 语义与清理混淆，dismiss 后条目被移出通知中心 [已解决]

1. 问题描述
   原 `NotificationClose` 同时承担“贴栏 dismiss”和“通知中心清理”两种动作：dismiss 时 daemon 直接把条目移出 `notifications` 并广播 `NotificationRemoved`，通知中心随之消失；用户要求 dismiss 只收起贴栏卡片，条目保留在中心等待清理。术语上“关闭通知”也应改为 dismiss。
2. 解决方法
   - 共享状态 `NotificationRecord` 增加 `dismissed`：dismiss 后置位，中心仍展示，贴栏弹窗过滤掉。
   - 请求拆分：`NotificationDismiss { id, reason }` 只 dismiss 贴栏卡片；`NotificationRemove { id }` 为通知中心清理单条；新增 `NotificationDismissAll` 供贴栏右键全部 dismiss；组关闭/全部清理保持清理语义。
   - 新增 `Push::NotificationDismissed(id)`：窗口只移除贴栏卡片，中心记录不动；`NotificationRemoved` 才彻底移除。
   - 发送方 `CloseNotification`、动作触发（非 `resident`）与超时都走 dismiss；`transient` 超时仍删除条目。
   - 通知中心单条关闭、组关闭、全部清理才向发送方 dismiss 并移除记录；已 dismissed 的条目清理时不再重复发送。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.5 通知弹窗 | 改 | 用户关闭统一改称 dismiss；dismiss 后条目保留通知中心，清理才移除；超时只收贴栏卡片（`transient` 删除）。 |
| `USER.html` | §5.2.6 通知中心 | 改 | 单条关闭 / 组关闭 / 全部清理属于 cleanup，与弹窗 dismiss 区分；dismiss 条目保留至 cleanup。 |
| `CORE.html` | §4.13 `info.rs` / §4.19 协议 | 改 | `NotificationRecord` 增 `dismissed`；Request 改为 `NotificationDismiss` / `NotificationRemove` / `NotificationDismissAll`；Push 增 `NotificationDismissed`。 |
| `ABOUT.html` | 「界面交互」通知弹窗 | 改 | 行为措辞 close→dismiss，并补“dismiss 后保留通知中心等待清理”。 |

4. 实际操作
   - `USER.html` §5.2.5：通知弹窗的“关闭”措辞统一改为 dismiss；补充 dismiss 与清理的分工（dismiss 只收贴栏卡片，清理才彻底移除），发送方 `CloseNotification` 按 dismiss 处理，超时只收贴栏卡片、`transient` 由 daemon 删除。
   - `USER.html` §5.2.6：单条 / 组 / 全部按钮改称清理，明确属于 cleanup；补充“dismiss 的条目保留在中心并继续显示，直到 cleanup”。
   - `CORE.html` §4.13 / §4.19 / §5.6：`NotificationRecord` 增加 `dismissed`；Request 改为 `NotificationDismiss` / `NotificationRemove` / `NotificationDismissAll` / `NotificationClearGroup` / `NotificationClear`；Push 增加 `NotificationDismissed`；`NotificationDismissReason` 替换旧 `NotificationCloseReason`；调用栈按 transient 删除、非 transient 置 `dismissed` 并广播 `NotificationDismissed` 重写。
   - `ABOUT.html`「界面交互」通知弹窗与通知中心：行为措辞 close→dismiss，组 / 单条 / 全部按钮改称清理，并补充“dismiss 后条目保留在通知中心等待清理”。
   - 仅改文档；`src/shared/protocol.rs`、`src/shared/model.rs` 与 `src/daemon/mod.rs` 的现有实现未改动。

### 21. 启动时不主动读取焦点窗口，标题区可能空白 [已解决]

1. 问题描述
   daemon 只在 `activewindow` / `activewindowv2` / `windowtitlev2` 事件到达时更新活动窗口；如果启动时已有焦点窗口，事件早已发生，`resync` 又不查询 `hyprctl activewindow`，窗口快照里的 `active` 就是默认空值，标题区空白直到下一次焦点事件。标题本身为空时也会直接显示空白。另一些拓扑变化（如把窗口移到别的工作区）不产生标题事件，窗口侧一旦漏推或显示为空，仅重同步且内容未变时也不会补发，标题永远不恢复。
2. 解决方法
   - 新增 `hyprland::active_window()`：读取 `hyprctl -j activewindow`（address / class / title，允许空字段）。
   - `resync` 主动刷新 `active_window` / `active_addr`，并**无条件重发** `StateDelta.active`；启动快照、移动窗口等触发重同步的场景都能恢复标题。
   - 标题标签在 `title` 为空时回退显示 `class`，避免已聚焦但标题空白。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.3 `daemon/hyprland.rs` | 改 | 增加 `active_window()` 签名与 `hyprctl -j activewindow` 字段说明。 |
| `CORE.html` | §5.1.1 启动调用栈 | 改 | `resync()` 补“读取当前焦点窗口并广播 active”。 |
| `USER.html` | §5.2.1 任务栏窗口与按钮 | 增 | 标题为空时回退显示应用 class，避免已聚焦窗口标题区空白。 |

4. 实际操作
   - `CORE.html` §4.3 增加 `ActiveWindowInfo` 与 `active_window()` 签名，注明读取 `hyprctl -j activewindow` 的 `address` / `class` / `title`，字段允许空串，无焦点或解析失败返回 `None`。
   - `CORE.html` §5.1.1 的 `Daemon::resync()` 调用栈插入 `hyprland::active_window()` 与无条件 `broadcast(StateDelta.active)`，覆盖启动前已有焦点与移动窗口等不产生标题事件的场景。
   - `USER.html` §5.2.1 补充标题为空时回退显示应用 class，避免已聚焦窗口标题区空白。
   - 仅改文档；`src/daemon/hyprland.rs` / `src/daemon/mod.rs` 现有主动读取与重发实现未改动。

### 22. daemon 在无显示器时随启动终端退出 [已解决]

1. 问题描述
   `bread daemon` 只做了简单的子进程派生，没有脱离启动终端会话；显示器断开导致终端会话结束时，daemon 会收到 SIGHUP 而退出。Hyprland 仍是显示器事件的唯一权威，daemon 只需保持存活并继续消费 `monitoradded` / `monitorremoved`。
2. 解决方法
   - `bread daemon` 后台化时用 `setsid` 脱离终端会话，并关闭子进程 stdin；stderr 仍继承启动终端用于日志。
   - 不增加显示器轮询或第二次显示器对账：插拔仍完全由 Hyprland 事件驱动。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §1 运行形态 | 改 | 注明后台化脱离终端会话，无显示器时 daemon 常驻。 |
| `CORE.html` | §3 SBOM | 改 | 增加 `libc`：`setsid` 脱离启动终端会话。 |
| `BUILD.html` | §3.1 运行命令 | 改 | 注明 `bread daemon` 脱离终端会话。 |
| `ABOUT.html` | 运行管理 | 改 | 注明无显示器时 daemon 继续后台常驻。 |

4. 实际操作
   - `src/main.rs` 的后台派生增加 `libc::setsid()` 与 `Stdio::null()` 作为 stdin。
   - `Cargo.toml` / `Cargo.lock` 增加 `libc` 依赖。
   - 未改动 `src/daemon/mod.rs` 与 `src/daemon/hyprland.rs`，显示器插拔继续以 Hyprland 事件为唯一来源。
   - 在隔离的 `XDG_RUNTIME_DIR` 下冒烟：无 Hyprland / 无显示器时 daemon 保持存活，`bread exit` 能正常清理退出。
   - 文档动作已在同一实现提交中落地：`CORE.html` §1 运行形态注明后台化脱离终端会话、无显示器时 daemon 常驻；§3 SBOM 增加 `libc` / `setsid`；`BUILD.html` §3.1 注明 `bread daemon` 脱离终端会话；`ABOUT.html`「运行管理」注明无显示器时继续后台常驻。本次仅补记状态，无需再改上述文档。

### 23. 通知中心组关闭按钮被展开手势吞掉 [已解决]

1. 问题描述
   通知中心的组标题同时承载展开/收起 `GestureClick` 与右侧关闭按钮。点击 `✕` 时，按下事件冒泡到组标题的 `pressed` 处理并触发 `rebuild()`；关闭按钮在 `released` 之前被销毁，`clicked` 不再派发，于是只看到分组展开/收起，`NotificationClearGroup` 请求没有发出。
2. 解决方法
   - 把组标题拆成“箭头 + 应用名”的展开区与独立的关闭按钮，`GestureClick` 只挂展开区。
   - 关闭按钮不再经过展开手势，继续发送 `Request::NotificationClearGroup { app_name }`；daemon 清理该组并广播 `NotificationCleared(Some(app_name))`。
   - 不改协议与 daemon 清理逻辑。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.6 通知中心 | 不动 | 组标题关闭按钮“清空本组全部消息”的语义已定义，本次只修实现。 |
| `CORE.html` | §4.13 `bar/modules/info.rs` | 不动 | 组标题结构属于实现细节，契约仍由 USER 描述。 |

4. 实际操作
   - `src/bar/modules/info.rs` 的 `group_header` 新增 `toggle` 容器（箭头 + 应用名），把展开/收起 `GestureClick` 从整个 `header` 移到 `toggle`；`✕` 保持独立 `clicked` 发送 `NotificationClearGroup`。
   - `cargo check` 通过。
   - `USER.html` / `CORE.html` 未改动；组关闭仍由 daemon 清理该应用全部通知并广播 `NotificationCleared(Some(app_name))`。
   - 该条两项文档动作均为“不动”，本次只补记状态：`USER.html` §5.2.6 的组清理语义与 `CORE.html` §4.13 的实现注记保持现状。

### 24. 通知中心组内消息残留外边距

1. 问题描述
   通知中心的通知组内部仍带外边距：外层内容容器 `gtk::Box` 使用 4px spacing，`.nc-panel .notif-card` 又保留 2px 上下 margin，导致同组标题、消息卡与「…」之间出现多余间距；用户要求组内不设 margin，只在组与组之间留一点间距。
2. 解决方法
   - 每个应用组包进独立的 `gtk::Box`，组内 spacing 设为 0，组间距由外层 `content` 的 4px spacing 提供。
   - `.nc-panel .notif-card` 的 margin 归零，移除 `.nc-group` 的 `margin-top: 2px`。
   - 组标题、消息卡、「…」按钮在组块内依次紧排，不再继承外层间距。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.6 通知中心 | 增 | 组内消息之间不设外边距；组与组之间以 4px 间距分隔。 |
| `CORE.html` | §4.13 `bar/modules/info.rs` | 增 | 每个应用组由 spacing 0 的容器承载，组间距由外层 content 的 4px spacing 提供。 |

4. 实际操作
   - `src/bar/modules/info.rs` 的 `CenterView::rebuild` 把每个应用组包进 `gtk::Box::new(gtk::Orientation::Vertical, 0)`，标题、消息卡与「…」都 append 到该组块，再整体 append 到 `content`。
   - `src/assets/bread.css` 将 `.nc-panel .notif-card` 的 margin 改为 `0`，并移除 `.nc-group` 的 `margin-top: 2px`；组间距仍由外层 `content` 的 4px spacing 提供。
   - `USER.html` §5.2.6 与 `CORE.html` §4.13 已补充上述契约；`cargo fmt --check` 与 `cargo check` 通过。

### 25. 去抖范围过大，工作区点击与滚轮切换被延后

1. 问题描述
   工作区 / 焦点事件原先全部折叠为 `Resync`，要等 `recv_timeout(TICK)` 的超时档才处理；点击工作区或滚轮切换后，工作区条最多晚一个去抖窗口才更新，用户体感为点击 / 滚轮被去抖。窗口打开 / 关闭、配置重载等纯显示变化可以合并，但用户输入触发的切换应立即反映。
2. 解决方法
   - 新增 `HyprlandEvent::WorkspaceChanged`，承载 `workspace*`、`focusedmon*`、`create/destroy/move/renameworkspace*`、`activespecial*`。
   - daemon 收到 `WorkspaceChanged` 立即 `resync()`，并清掉已覆盖的 `pending_resync`；点击与滚轮切换不再等待滴答。
   - 仅 `configreloaded` / `changefloatingmode` 等纯显示事件继续走 `Resync`，由 `TICK` 合并。
   - 单元测试拆分：工作区事件断言 `WorkspaceChanged`，配置重载 / 浮动模式断言 `Resync`。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.3 `HyprlandEvent` | 改 | 增加 `WorkspaceChanged`，并注明工作区 / 焦点事件直达、不进入去抖窗口。 |
| `CORE.html` | §5.1.1 daemon 启动 | 增 | 注明 `TICK` 只合并 `Resync` 类纯显示事件，工作区事件收到即重同步。 |

4. 实际操作
   - `src/daemon/hyprland.rs` 增加 `WorkspaceChanged` 枚举，工作区 / 焦点类事件从 `Resync` 拆出；配置重载与浮动模式仍保留在 `Resync`。
   - `src/daemon/mod.rs` 的 `on_hyprland` 收到 `WorkspaceChanged` 时清 `pending_resync` 并立即 `resync()`，使点击 / 滚轮切换即时反馈。
   - `src/daemon/hyprland.rs` 更新解析测试：工作区事件断言 `WorkspaceChanged`，配置重载 / 浮动模式断言 `Resync`。
   - `CORE.html` §4.3 与 §5.1.1 已补充事件分层与去抖范围说明；`cargo fmt --check`、`cargo check` 与相关解析测试通过。

### 26. 工作区滚轮无合并窗口，相对切换乱序并吞操作 [已解决]

1. 问题描述
   移除统一去抖后，工作区条的每个 GTK 滚动事件都会 `spawn` 一个线程发送一次 `m±1` 请求。平滑滚动 / 高频滚轮会在极短间隔内产生多个事件，工作区被连续切过多个；每次切换各起一线，相对目标（`m+1` / `m-1`）还可能在线程调度中后发先至，最终落点与滚动方向不符，表现为操作被吞。用户要求只对滚轮做极小间隔合并，不恢复任何统一去抖。
2. 解决方法
   - 新增 `ScrollGate`：首个滚动事件立即放行；从该事件起 20ms 内的同向事件忽略（合并为一次切换）；反向事件立即放行并以该事件重新计时；被忽略的事件不刷新窗口。
   - 方向改为取位移绝对值更大的轴（向下 / 向右为 `m+1`，向上 / 向左为 `m-1`），零位移事件忽略，避免斜向滚动的正负分量被 `||` 误判。
   - 新增 `SwitchQueue`：点击与滚轮统一提交到单工作线程，按提交顺序执行工作区切换；不再逐事件 `spawn`，相对切换不会乱序，也避免高频滚动堆出并发请求。
   - 点击路径不做任何去抖，仍保持“按下与松开之间不重建按钮”的既有增量对账。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.12 `bar/modules/workspace.rs` | 改 | `WorkspaceWidget` 的 `ipc` 字段改为 `switches: Rc<SwitchQueue>`；补充 `ScrollGate` 的 20ms 同向合并、反向立即放行与重新计时语义，以及切换请求按提交顺序串行执行。 |
| `USER.html` | §5.2.2 Workspace 按钮 | 增 | 补可验收行为：滚轮向下 / 向右切到下一工作区、向上 / 向左切到上一工作区；同向 20ms 内的连续滚动合并为一次，反向滚动立即生效，点击不受合并窗口影响。 |

4. 实际操作
   - `src/bar/modules/workspace.rs` 新增 `SCROLL_MERGE`（20ms）、`ScrollGate` 与 `scroll_direction`；滚动回调只在 `accept` 放行时提交 `m±1`，点击与滚轮共用 `SwitchQueue` 的单工作线程。
   - `SwitchQueue` 线程命名为 `workspace-switch`，请求失败沿用 `[workspace] switch failed` 日志；队列关闭时记录 `[workspace] switch queue closed`。
   - 新增 3 个单元测试，覆盖方向判定（含斜向与零位移）、同向合并窗口（被忽略事件不刷新计时）以及反向放行后重新计时。
   - `CORE.html` §4.12 与 `USER.html` §5.2.2 已补充上述契约；`cargo fmt --check`、`cargo check` 与 `cargo test --bin bread scroll` 通过。

### 27. dbusmenu 子菜单三角取 muted，在 fg 底上对比度不足

1. 问题描述
   自绘菜单弹窗以 `fg`（浅色）为底、`bg`（深色）为文字，子菜单指示三角却单独取 `muted`（浅灰蓝）。`muted` 与 `fg` 明度接近，三角在浅色面板上几乎融进背景，带子菜单的条目缺少可辨识的展开提示。
2. 解决方法
   - `.menu-arrow` 的颜色从 `muted` 改为 `bg`，与菜单文字同色，借助深色前景在 `fg` 底上取得足够对比度。
   - 保留 Hover 行的既有覆盖：`.menu-row:hover .menu-arrow` 仍取 `cream`，与 Hover 文字一致。
   - 只改颜色，不改三角字形、尺寸与布局；禁用行的 45% 不透明度继续作用于整行。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.3 托盘与托盘菜单 | 改 | 子菜单箭头颜色由 `muted` 改为 `bg`，并注明与文字同色以保证 `fg` 底上的对比度。 |
| `CORE.html` | §4.16 `bar/modules/tray.rs` | 不动 | 箭头仍为 `▸` + `menu-arrow` 样式类，渲染结构不变，仅主题色调整。 |

4. 实际操作
   - `src/assets/bread.css` 的 `.menu-arrow` 由 `color: @muted` 改为 `color: @bg`；Hover 覆盖规则未动。
   - `USER.html` §5.2.3 的菜单配色说明同步改为子菜单箭头取 `bg`，并注明对比度理由。
   - `CORE.html` 未改动；箭头节点与样式类保持现状。
   - 本次只改样式与文档；`cargo fmt --check`、`cargo check` 与 `cargo test --bin bread` 通过。

### 28. dbusmenu 多级子菜单互相顶掉，并在无根焦点时触发 GTK CRITICAL [已解决]

1. 问题描述
   打开“子菜单的子菜单”时上级菜单会被关闭，悬停展开的子层还会与上级互相抢焦点。三个原因叠加：
   - 所有子层都以同一个组标签 `tray:<item>:sub` 注册；`PopupManager::open` 会先关同组旧实例，于是打开第三级时把第二级一并关掉。
   - 所有子层都 `attach` 到根弹窗而不是所在行的上级弹窗；第三级与第二级成为同级 grab 弹窗，`close_subs_not_at` 的祖先判定也失效，进入第三级会把第二级当作“不在祖先链上”收起。
   - 菜单行全部不可聚焦且弹窗内没有其它可聚焦子项。GTK 的 `gtk_popover_focus` 在 autohide 弹窗打开时若根焦点为空，会把 NULL 传给 `gtk_widget_is_ancestor` 并打印 `Gtk-CRITICAL`（属于 GTK 内部缺空值检查，但可由焦点落点规避）。
2. 解决方法
   - 子层改为挂到 `containing_popover(anchor)` 找到的上级弹窗：根行挂根层，二级行挂二级层，逐级嵌套；锚行脱离当前根会话时丢弃迟到回执。
   - 每个子层分配递增序号，组标签改为 `tray:<item>:sub:<id>`；`closed` 只捕获序号除名，避免 closure 自持 popover。
   - `close_subs_not_at` 改用自走的 `parent()` 链判断祖先，并在判定前剔除已隐藏的迟到条目；子层按由深到浅顺序 popdown，`PopupManager::close_all` / `close_by` 同样反序关闭。
   - 菜单层 `set_focusable(true)` 作为弹窗焦点落点（行仍不可聚焦），并加 `.menu-layer { outline: none; }`；根菜单被外部关闭时一并清理残留子层。
   - `build_menu_layer` 不再清空全局 `pending_rows`（会误伤其它行的飞行中展开）；改在根菜单重读重建前作废。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.16 `bar/modules/tray.rs` | 增 | 子层逐级挂上级弹窗、每层独立组标签；祖先判定自走 parent 链并由深到浅关闭；菜单层可聚焦以规避 GTK 空焦点 CRITICAL。 |
| `USER.html` | §5.2.3 托盘与托盘菜单 | 增 | 子菜单逐级嵌套，进入下一级不关闭上级；悬停其它条目只收起相关子级。 |
| `ABOUT.html` | 界面交互 | 改 | 托盘菜单交互补“子菜单逐级嵌套，进入更深一层不关闭上级”。 |

4. 实际操作
   - `src/bar/modules/tray.rs` 新增 `SubMenu { id, popover, anchor }`、`next_sub_id`；`open_sub_popover` 按锚行实际所属弹窗 `attach` 与 `point_at_expanded`，组标签带序号；`closed` 仅按序号除名。
   - 新增 `widget_within` / `containing_popover`；`close_subs_not_at` 改用前者，反向 popdown，并剔除不可见迟到条目；根菜单 `closed` 清理残留子层。
   - `build_menu_layer` 将内容层设为可聚焦并加 `menu-layer` 类，移除全局 `pending_rows.clear()`；`on_menu_read_done` 在重建前清空 pending。
   - `src/bar/modules/popup.rs` 的 `close_all` / `close_by` 改为反序关闭；`src/assets/bread.css` 增加 `.menu-layer { outline: none; }`。
   - `CORE.html` §4.16、`USER.html` §5.2.3、`ABOUT.html` 已同步契约；`cargo fmt --check`、`cargo check`、`cargo clippy --all-targets` 与 `cargo test --bin bread` 通过。

### 29. 子菜单弹层错误高亮末行（合成 enter），motion-only 又会锁死父菜单

1. 问题描述
   悬停父条目展开子菜单后，子菜单会错误高亮一个选项（常见为末行），若该行还有子菜单会继续递归展开。原因是合成器映射 xdg_popup 时可能附带坐标错误的指针 crossing，GTK 向错误行派发合成 `enter`，而原实现既用 GTK `:hover` 显示高亮、又用 `enter` 启动展开。
   但把悬停整体改成 `motion` 驱动也不行：子菜单 popover 是 GTK grab widget，`gtk_main_do_event` 会把非后代的 motion 事件重定向给 grab widget，父菜单行从此收不到 motion，无法快速切换到其它子菜单；`enter/leave` 在 picking 阶段处理，不受 grab 重定向。
2. 解决方法
   - 父菜单切换与子层展开继续由 `enter` 驱动；`motion` 仅补高亮和补展开计时（应对合成 enter 已占住指针焦点、真实移入同一行不再发 enter 的情况）。
   - 每层菜单行记录 80ms 悬停静默截止：静默期内该层行忽略 enter/motion，吞掉映射期的错误 crossing；父菜单行属于旧层，不受新层静默影响。
   - 高亮改为应用自管的 `menu-hover` 类，CSS 将 `.menu-row:hover` 置为透明，阻断 GTK PRELIGHT 的错误着色。
   - `hovered_row` 保证同一时刻至多一行高亮，并在菜单重读/关闭时清除。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.16 `bar/modules/tray.rs` | 增 | 父菜单切换由 enter 驱动、motion 仅补高亮；新层 80ms 静默；高亮自管 `menu-hover` 且至多一行。 |
| `USER.html` | §5.2.3 托盘与托盘菜单 | 增 | 弹层打开不预选选项，高亮随真实指针；子菜单打开后父菜单仍可立即切换。 |

4. 实际操作
   - `src/bar/modules/tray.rs` 新增 `HOVER_SETTLE`（80ms）与 `hovered_row`；`build_menu_layer` / `build_menu_row` 接收该层的静默截止时刻，根层与子层新行取 `now + HOVER_SETTLE`，菜单重读重建取 `now`。
   - 行控制器恢复 `connect_enter`（切换父菜单、启动展开计时）并新增 `connect_motion`（仅补高亮/补计时）；`highlight_row`、`schedule_expand`、`clear_menu_hover` 统一高亮与驻留逻辑。
   - `src/assets/bread.css` 将 `.menu-row:hover` 改为透明，新增 `.menu-row.menu-hover` 及子部件配色；`CORE.html` §4.16、`USER.html` §5.2.3 同步契约。
   - `cargo fmt --check`、`cargo check`、`cargo clippy --all-targets` 与 `cargo test --bin bread` 通过。

### 30. 托盘菜单持续打印 Gtk-CRITICAL，并把迟到展开回执当成异常

1. 问题描述
   菜单操作时持续出现 `gtk_widget_is_ancestor: assertion 'GTK_IS_WIDGET (widget)' failed`。该断言来自 GTK popover 内部：autohide 弹窗打开时若根窗口焦点为空，`gtk_popover_focus`（以及 `activate_default`）会把 NULL 传给 `gtk_widget_is_ancestor`。此前把菜单层设为可聚焦，但弹窗关闭后 GTK 会清掉位于弹窗内的焦点，下一次打开仍是 NULL，所以每个菜单都会报。
   另外，悬停计时与点击可能对同一 `(item, entry)` 各发一次展开请求；第一个回执消费锚行后，第二个迟到回执被当成 `unexpected expand reply` 打印整份菜单。
2. 解决方法
   - 栏根 `CenterBox` 设为可聚焦并在启动后 `grab_focus`，作为常驻焦点落点；菜单层恢复不可聚焦，弹层开关不再移动焦点。
   - `PopupManager::open` 打开前检查根焦点，为空则重挂窗口子控件，防止其它控件销毁时焦点被清空。
   - `request_expand` 对同 `(item, entry)` 的飞行中请求去重；`on_expand_done` 对“Menu + 锚行已失效”的迟到回执静默丢弃。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `CORE.html` | §4.16 `bar/modules/tray.rs` | 改 | 菜单层/行/图标均不可聚焦，栏根常驻焦点；`PopupManager::open` 前兜底；展开请求去重、迟到回执静默。 |
| `USER.html` | §5.2.3 托盘与托盘菜单 | 不动 | 既有交互契约不变，仅内部焦点与回执处理调整。 |

4. 实际操作
   - `src/bar/mod.rs` 的栏根 `CenterBox` 增加 `bar-root` 类并 `set_focusable(true)`，`present()` 后经 idle `grab_focus`；`src/assets/bread.css` 增加 `.bar-root { outline: none; }`。
   - `src/bar/modules/tray.rs` 移除菜单层的 `set_focusable(true)` 与 `menu-layer` 类；`on_expand_done` 对 `Menu + None` 静默，`request_expand` 对同键在飞请求去重。
   - `src/bar/modules/popup.rs` 新增 `ensure_root_focus`，在 `open` 前兜底。
   - `CORE.html` §4.16 已同步；`cargo fmt --check`、`cargo check`、`cargo clippy --all-targets` 与 `cargo test --bin bread` 通过。

### 31. 控制中心保留外层气泡，主背景 / 滑轨 / 日历边框不符合样式要求 [已解决]

1. 问题描述
   控制中心的 `GtkPopover` 仍使用 GTK 默认气泡：带箭头、主题背景与描边；面板自身又叠加了 `gold` 边框，形成“外面一圈气泡”的双层轮廓。面板主背景取 `surface`，滑轨取 `muted`，日历带 `gold` 边框，与“背景用 `bg`、滑轨用 `surface`、日历不要边框”的要求不符。
2. 解决方法
   - 弹窗加 `center-popover-shell` 类并 `set_has_arrow(false)`；`shell` 与 `contents` 设为透明、无边框、无阴影、无内边距，可见卡片只由内层 `panel` 绘制。
   - `.center-popover` 背景从 `surface` 改为 `bg`，移除 `gold` 描边，保留圆角与内边距。
   - 滑块 `trough` 背景从 `muted` 改为 `surface`，`highlight` 仍取 `fg`。
   - 日历移除边框，保留 `surface` 背景、圆角与今天高亮。
3. 修订建议

| 文档名 | 章节 | 动作 | 内容 |
|--------|------|------|------|
| `USER.html` | §5.2.4 控制中心 | 改 | 面板改为 `bg` 背景、无外层气泡/箭头/描边；滑轨改 `surface`；日历改无边框。 |
| `CORE.html` | §4.17 `bar/modules/center.rs` | 改 | 增加透明弹窗外壳与无箭头说明，同步面板 / 滑轨 / 日历取色。 |

4. 实际操作
   - `src/bar/modules/center.rs` 的 popover 增加 `center-popover-shell` 类并关闭箭头。
   - `src/assets/bread.css` 增加 `.center-popover-shell` / `contents` 透明规则；`.center-popover` 改 `bg` 背景并去边框；滑轨改 `surface`；日历去边框。
   - `USER.html` §5.2.4 与 `CORE.html` §4.17 已同步契约。
   - `cargo fmt --check`、`cargo check`、`cargo clippy --all-targets` 与 `cargo test --bin bread` 通过。
