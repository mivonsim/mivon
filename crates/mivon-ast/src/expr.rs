use mivon_core::intern::Symbol;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    Value(Value),
    FillLit(mivon_core::LogicVal),
    Ident {
        name: Symbol,
        line: usize,
        col: usize,
    },
    FuncCall {
        name: Symbol,
        args: Vec<Expr>,
        /// Posisi source pemanggilan (`func(...)`) untuk diagnostic col/line.
        line: usize,
        col: usize,
    },
    RangeSelect {
        expr: Box<Expr>,
        msb: Box<Expr>,
        lsb: Box<Expr>,
    },
    BitSelect {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    PartSelect {
        expr: Box<Expr>,
        base: Box<Expr>,
        width: Box<Expr>,
    },
    Concat(Vec<Expr>),
    Replicate {
        count: Box<Expr>,
        expr: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    BinaryOp {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    TernaryOp {
        cond: Box<Expr>,
        true_expr: Box<Expr>,
        false_expr: Box<Expr>,
    },
    Paren(Box<Expr>),
    String(String),
    MethodCall {
        obj: Box<Expr>,
        method: Symbol,
        args: Vec<Expr>,
        with_clause: Option<Box<Expr>>,
    },
    MemberAccess {
        obj: Box<Expr>,
        field: Symbol,
    },
    Null,
    Inside {
        expr: Box<Expr>,
        range_list: Vec<Expr>,
    },
    StreamingConcat {
        op: String, // ">>" or "<<"
        slice_size: Option<Box<Expr>>,
        slices: Vec<Expr>,
    },
    /// Struct assignment pattern `'{name: value, ...}` (juga `default:` dan
    /// anggota posisional). Dipakai untuk nilai struct/union dalam default
    /// parameter/localparam serta inisialisasi. Sebelumnya pola bernama
    /// di-discard menjadi `FillLit(0)` — sekarang dipertahankan agar member
    /// access (`PartInfo[k].offset`) bisa di-const-eval.
    StructLit {
        members: Vec<StructLitMember>,
    },
    Cast {
        dtype: Symbol,
        expr: Box<Expr>,
    },
    /// Cast dengan width dari ekspresi: `size'(expr)` — mis. `$clog2(N)'(x)`
    /// (casting_type `constant_primary` per LRM 1800). Parser postfix membuat
    /// variant ini saat menemukan `Quote` setelah ekspresi umum (bukan nama
    /// tipe / literal). Elaborator mengevaluasi width via const_eval.
    CastWidth {
        width: Box<Expr>,
        expr: Box<Expr>,
    },
    ScopedIdent {
        package: Symbol,
        item: Symbol,
        /// Posisi source (`pkg::item`) untuk diagnostic col/line.
        line: usize,
        col: usize,
    },
    Dist {
        expr: Box<Expr>,
        items: Vec<DistItem>,
    },
    /// Increment/decrement dalam ekspresi (LRM 1800 §11.4.1):
    /// `i++`, `i--`, `++i`, `--i`. Operand adalah lvalue yang DITULIS ULANG
    /// saat ekspresi dievaluasi (side-effect) — `pre` menentukan nilai hasil
    /// (baru untuk prefix, lama untuk postfix). Parser menghasilkan varian ini
    /// untuk kejadian di level ekspresi; kejadian statement-level (`i++;`
    /// baris sendiri) sudah di-desugar parser jadi `BlockingAssign` biasa.
    IncDec {
        expr: Box<Expr>,
        /// true = `++` (tambah 1), false = `--` (kurang 1)
        inc: bool,
        /// true = prefix (`++i` → hasil nilai BARU), false = postfix (`i++`
        /// → hasil nilai LAMA)
        pre: bool,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StructLitMember {
    /// Anggota bernama: `name: value`.
    Named(Symbol, Expr),
    /// Anggota posisional: `value` (tanpa label).
    Positional(Expr),
    /// `default: value` — nilai untuk field yang tidak disebutkan.
    Default(Expr),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DistItem {
    Value(Box<Expr>, DistWeight),
    Range(Box<Expr>, Box<Expr>, DistWeight),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DistWeight {
    Item(u64),  // := weight (each item in range gets this weight)
    Range(u64), // :/ weight (total weight for the range)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    Binary {
        bits: String,
        width: Option<usize>,
        is_signed: bool,
    },
    Decimal(i64),
    Hex {
        bits: String,
        width: Option<usize>,
        is_signed: bool,
    },
    Octal {
        bits: String,
        width: Option<usize>,
        is_signed: bool,
    },
    Real(f64),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UnaryOp {
    Plus,
    Minus,
    BitNot,
    Not,
    ReductionAnd,
    ReductionNand,
    ReductionOr,
    ReductionNor,
    ReductionXor,
    ReductionXnor,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Power,
    Eq,
    Neq,
    CaseEq,
    CaseNeq,
    EqWild,
    NeqWild,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    Sshl,
    Sshr,
    BitAnd,
    BitOr,
    BitXor,
    BitXnor,
    LogicalAnd,
    LogicalOr,
}
