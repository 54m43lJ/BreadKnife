//! GTK 整合层（渲染直出；事件消费模式见 ARCHITECTURE §2.2，桥接代码归消费方）。

mod paintable;

pub use paintable::icon_paintable;
