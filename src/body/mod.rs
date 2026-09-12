//! 体そのもの。Wayland も入力も知らない純粋なデータ構造とロジックだけを置く。

pub mod animal;
pub mod field;
pub mod lenia;
pub mod lenia_body;
pub mod perturbation;
pub mod port;

pub use animal::{load_animal, Animal};
pub use field::{Field, FieldView};
pub use lenia::Lenia;
pub use lenia_body::LeniaBody;
pub use perturbation::{CellPos, Perturbation};
pub use port::BodyPort;
