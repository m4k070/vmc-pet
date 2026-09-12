//! 体が外界に見せる唯一の窓口。
//!
//! 外から体にできることは「摂動を注入する」ことと「場を読む」ことだけで、
//! 内部の更新規則に触る経路はない。将来コントローラ(知能側)を後付けするときも、
//! 人間の入力と同じこの窓口を通す。

use super::{FieldView, Perturbation};

/// 体の境界。
pub trait BodyPort {
    /// 場に局所的な摂動を注入する。
    fn inject(&mut self, perturbation: Perturbation);

    /// 場を読む。読み取り専用ビューなので、ここから場を書き換えることはできない。
    fn observe(&self) -> FieldView<'_>;
}
