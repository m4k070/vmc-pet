//! 体そのもの。Wayland も入力も知らない純粋なデータ構造とロジックだけを置く。

pub mod animal;
pub mod field;
pub mod lenia;

pub use animal::load_animal;
pub use field::{Field, FieldView};
pub use lenia::Lenia;
