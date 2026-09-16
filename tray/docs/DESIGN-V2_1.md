Linux 系统托盘工具链
工具链包含两个库 crate（breadpan、boulanger）

# 总体
### 项目身份
| 项 | 决策 |
|---|---|
| 项目 | `breadpan`（SNI/DBusMenu 托盘客户端库）+ `boulanger`（SNI Watcher 库 |
| 命名逻辑 | breadpan=烤盘（承放、供取用）；boulanger=法语面包师（照看炉火、收发面团＝登记 Item） |
| 发布状态 | 暂不发布 crates.io；版本策略照常 |
| 使用场景 | Wayland 会话；**强绑定 GTK**（整套项目面向 GTK 编写，不支持跨框架移植） |
| 目录形态 | 同 BreadKnife 目录下平行 crate；`breadpan ⊥ boulanger`，互相零依赖 |
### breadpan 定位与边界
**是什么**：无头的 SNI 协议客户端库 + DBusMenu 客户端库，以统一事件流交付托盘世界的全部变化；独家支持 GTK，为 GTK 应用提供深度整合（渲染对象直出 + 事件桥接方案）。**不实现 Watcher 角色**（由 boulanger 承担，breadpan 仅作为客户端观察其存亡）。

**不是什么**：

| 不做 | 理由 |
|---|---|
| 拥有任何 UI（窗口/布局/事件循环） | 渲染逻辑归消费方；但数据形态深度整合 GTK |
| XEmbed/X11 传统托盘 | 场景钉死 Wayland |
| 菜单渲染/弹出定位 | 交付 MenuSnapshot 数据模型与激活接口 |
| AttentionMovieName | 实际使用者寥寥，进「已知不实现清单」 |
| 异步方法 | 公开 API 零 async；内部用 zbus 自带执行器 |
| 菜单缓存 | 内容一致性代价 >> 缓存收益 |
# 内部
### 设计原则（正文逐条附"为什么"）
2. **阻塞优先**：零 async 泄漏，内部线程模型对使用者不可见；
3. **事件通知 + 主动拉取快照**（不推大对象）；
4. **容错降级**：永不 panic，坏 Item 不拖垮整体，单 Item 死亡隔离；
5. **依赖白名单**：`zbus`+ `thiserror` + **`gtk4`** + `std`；
6. **规范化兜底 + raw 逃生门**：对可确定性识别的协议偏差，库有责任转化为规范行为；识别不确定处保持沉默（宁可原样交付，不可错误"纠正"）；所有经过解析的字段同时保留 raw 原始字段，供客户端特殊处理（例：`MenuItem.raw_label`、`Icon` 双份数据）。
**硬约束**：
- a. 零反向依赖：breadpan/boulanger 的 Cargo.toml 不出现 BreadKnife 内任何成员；CI 隔离目录 `cargo build -p` 验证；
- b. 零下游概念泄漏。
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