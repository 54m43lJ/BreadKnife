# bnote 文档合规结果

- 日期：2026-10-19
- 基准：`GUIDELINES.md`
- 范围：`bnote/README.html`、`USER.html`、`CORE.html`、`PROVIDER.html`、`diagram/`、`reference/`
- 结论：**无未关闭项。**

## 已完成

- P0：`expire_timeout` 语义、`hints`/`Notify` 参数拆分、`Session`/`Error` 边界、SBOM/Cargo、协议图回程、CORE 调用栈、USER §4.2 结构。
- P1：bnote 自持连接/运行时；事件异步、命令同步；Provider 三要素/类型映射/错误表；§4.3 入站/出站两张表；reference 分章清理与引用修复；示例契约清理。
- P2：占位段、CONVENTIONS 格式、协议名归属、`Notice.id` 语义、Credits/非目标、Provider 标题、规范引用、草案标记。

## 说明

- `examples/` 是代码/原型，不属于文档合规审查范围，已从分支删除。
- C-15（注释预算）原位置基于重构前章节编号，当前 `CORE §4.6` 为 `src/signal.rs`，不成立；已撤销。
- 当前参考文档为 `reference/2.basic-design.html` 至 `reference/9.dbus-protocol.html`，入口 `reference/index.html`。
