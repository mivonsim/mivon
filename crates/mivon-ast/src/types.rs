use std::collections::HashMap;

use super::expr::Expr;
use super::stmt::{AlwaysBlock, InitialBlock, Stmt};
use mivon_core::intern::Symbol;

// Re-export constant evaluation functions
pub use crate::const_eval::{const_eval_simple, const_eval_with_params, string_to_i64};

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Design {
    pub modules: Vec<Module>,
    pub classes: Vec<ClassDecl>,
    pub packages: Vec<PackageDecl>,
    pub interfaces: Vec<Interface>,
    pub binds: Vec<BindDecl>,
    pub clocking_blocks: Vec<ClockingBlock>,
    pub configs: Vec<ConfigDecl>,
    pub udp_defs: Vec<UdpDef>,
    pub top_module: Option<Symbol>,
    pub unit_imports: Vec<(Symbol, Symbol)>,
    pub unit_decls: Vec<Decl>,
    pub unit_funcs: Vec<FunctionDecl>,
    pub unit_tasks: Vec<TaskDecl>,
    pub unit_typedefs: Vec<TypedefDecl>,
    pub unit_params: Vec<ParamDecl>,
    pub timescale: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConfigDecl {
    pub name: Symbol,
    pub design_top: Option<Symbol>,
    pub default_liblist: Option<String>,
    pub rules: Vec<ConfigRule>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConfigRule {
    InstanceLiblist { instance: Symbol, liblist: String },
    CellLiblist { cell: Symbol, liblist: String },
    UseLiblist { liblist: String },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BindDecl {
    pub target: Symbol,
    pub instance: ModuleInstance,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassDecl {
    pub name: Symbol,
    pub extends: Option<Symbol>,
    pub type_params: Vec<TypeParam>,
    pub members: Vec<ClassMember>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TypeParam {
    pub name: Symbol,
    pub default_type: Option<DataType>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClassMember {
    Decl(Decl),
    Function(FunctionDecl),
    Task(TaskDecl),
    Constraint {
        name: Symbol,
        body: Vec<ConstraintItem>,
        /// LANG-32: `static constraint` — block dibagi antar SEMUA instance
        /// class; constraint_mode()-nya berlaku global per-class, bukan
        /// per-instance (IEEE 1800-2017 §18.5.10). serde(default) agar AST
        /// cache MICD lama (tanpa field ini) tetap bisa di-restore.
        #[serde(default)]
        is_static: bool,
    },
    /// LANG-40: `let` di dalam class (IEEE 1800-2017 §11.12.2).
    Let(LetDecl),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskDecl {
    pub name: Symbol,
    pub ports: Vec<FunctionPort>,
    pub decls: Vec<Decl>,
    pub stmts: Vec<Stmt>,
    pub virtual_flag: bool,
    pub is_static: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConstraintItem {
    Expr(Expr),
    SolveBefore {
        vars: Vec<Symbol>,
    },
    /// `if (cond) { items } else { items }` — constraint kondisional (F12).
    /// Solver mengevaluasi cond lalu menerapkan hanya cabang yang terpenuhi.
    If {
        cond: Expr,
        then: Vec<ConstraintItem>,
        els: Vec<ConstraintItem>,
    },
    /// LANG-31: `soft expr;` — constraint soft (best-effort). Solver
    /// mengevaluasinya, tetapi pelanggaran TIDAK membuat randomize gagal:
    /// soft constraint boleh dilanggar bila bertentangan dengan hard
    /// constraint (IEEE 1800-2017 §18.5.14).
    Soft(Expr),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Module {
    pub name: Symbol,
    pub ports: Vec<Port>,
    pub params: Vec<ParamDecl>,
    pub decls: Vec<Decl>,
    pub items: Vec<ModuleItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModportItem {
    pub name: Symbol,
    pub direction: PortDirection,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Modport {
    pub name: Symbol,
    pub items: Vec<ModportItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Interface {
    pub name: Symbol,
    pub params: Vec<ParamDecl>,
    pub ports: Vec<Port>,
    pub decls: Vec<Decl>,
    pub items: Vec<ModuleItem>,
    pub modports: Vec<Modport>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Port {
    pub name: Symbol,
    pub direction: PortDirection,
    pub range: Option<Range>,
    pub expr_range: Option<ExprRange>,
    pub dtype_name: Option<Symbol>,
    /// Unpacked array dimension `[msb:lsb]` (atau `[N]` → [N-1:0]) pada port.
    pub array_range: Option<Range>,
    /// Dimensi unpacked LANJUTAN `[a][b]` setelah dimensi pertama
    /// (`array_range`) pada port — `input logic [7:0] mat [0:1][0:1]`.
    /// Pasangan (range ter-resolve saat parse, ekspresi ukuran utk parameter).
    pub extra_unpacked_dims: Vec<(Option<Range>, Option<Expr>)>,
    /// Dimensi packed tambahan sebelum nama port: `[a:b][c:d] name`.
    /// Dimensi pertama di `range`/`expr_range`, sisanya di sini.
    pub extra_packed_dims: Vec<ExprRange>,
    /// Initializer port ANSI: `output reg [7:0] b = 8'h2A;` — SV legal.
    /// Elaborator menerjemahkannya ke `Process::Initial` (seperti initializer
    /// deklarasi `reg b = 8'h2A;`). None = port tanpa default value.
    pub init_expr: Option<Expr>,
}

impl Port {
    pub fn resolved_width(&self, param_vals: &HashMap<Symbol, i64>) -> Result<usize, String> {
        let base = if let Some(r) = &self.range {
            r.width()
        } else if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            r.width()
        } else {
            1
        };
        let mut total = base;
        for er in &self.extra_packed_dims {
            let r = resolve_expr_range(er, param_vals)?;
            total = total.saturating_mul(r.width());
        }
        Ok(total)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PortDirection {
    Input,
    Output,
    Inout,
    Ref,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Range {
    pub msb: usize,
    pub lsb: usize,
}

impl Range {
    pub fn width(&self) -> usize {
        if self.msb >= self.lsb {
            self.msb.saturating_sub(self.lsb).saturating_add(1)
        } else {
            self.lsb.saturating_sub(self.msb).saturating_add(1)
        }
    }
}

// ============================================================================
// CATATAN: impl DataType dan impl DeclKind dipindahkan ke sini dari
// src/elaboration/elaborator.rs untuk memisahkan tanggung jawab.
// DataType dan DeclKind adalah tipe AST, jadi method-methodnya
// seharusnya berada di file definisi tipe (ast/types.rs), bukan di elaborator.
// ============================================================================

impl DataType {
    /// Mengembalikan lebar (width) default untuk tipe data ini.
    pub fn width(&self) -> usize {
        match self {
            DataType::Bit | DataType::Logic => 1,
            DataType::Byte => 8,
            DataType::Shortint => 16,
            DataType::Int | DataType::Integer => 32,
            DataType::Longint => 64,
            DataType::Time => 64,
            DataType::Real | DataType::Realtime => 64,
            DataType::String => 0,
            DataType::Signed(inner) => inner.width(),
            DataType::UserDefined(_) => 64,
            DataType::EnumType {
                base: _,
                members: _,
            } => 32,
            DataType::StructType { members } => members
                .iter()
                .map(|m| m.range.as_ref().map(|r| r.width()).unwrap_or(1))
                .sum(),
            DataType::UnionType { members } => members
                .iter()
                .map(|m| m.range.as_ref().map(|r| r.width()).unwrap_or(1))
                .max()
                .unwrap_or(1),
            DataType::Void => 0,
        }
    }
}

impl DeclKind {
    /// Mengembalikan lebar default untuk deklarasi jenis ini.
    /// Contoh: wire/reg/logic default 1-bit, int/integer default 32-bit.
    pub fn default_width(&self) -> usize {
        match self {
            DeclKind::Wire
            | DeclKind::Reg
            | DeclKind::Logic
            | DeclKind::Wand
            | DeclKind::Wor
            | DeclKind::Tri
            | DeclKind::Tri0
            | DeclKind::Tri1
            | DeclKind::TriAnd
            | DeclKind::TriOr
            | DeclKind::Supply0
            | DeclKind::Supply1 => 1,
            DeclKind::Int | DeclKind::Integer => 32,
        }
    }
}

/// Cek apakah tipe data signed (single source of truth — dipakai elaborator
/// utk is_signed signal/field dan simulator utk perbandingan/div/mod signed).
/// IEEE 1800 §6.11: int/integer/byte/shortint/longint intrinsik signed;
/// `Signed(_)` eksplisit; sisanya unsigned (bit/logic/time/real/...).
pub fn is_signed_type(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::Signed(_)
            | DataType::Int
            | DataType::Integer
            | DataType::Byte
            | DataType::Shortint
            | DataType::Longint
    )
}

/// A range whose bounds are expressions (may reference parameters).
/// Resolved during elaboration once parameter values are known.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExprRange {
    pub msb: Expr,
    pub lsb: Expr,
}

pub fn resolve_expr_range(
    er: &ExprRange,
    param_vals: &HashMap<Symbol, i64>,
) -> Result<Range, String> {
    let msb = const_eval_with_params(&er.msb, param_vals)?;
    let lsb = const_eval_with_params(&er.lsb, param_vals)?;
    // Range negatif VALID di SystemVerilog (mis. `[-1:0]` = 2 bit). Offset
    // index minimum ke 0 — width & arah range tetap, representasi usize aman
    // (tidak mem-blow-up alokasi sinyal). Contoh: [-1:0] → [0:1] (msb<lsb).
    if msb < 0 || lsb < 0 {
        let offset = -msb.min(lsb);
        return Ok(Range {
            msb: (msb + offset) as usize,
            lsb: (lsb + offset) as usize,
        });
    }
    Ok(Range {
        msb: msb as usize,
        lsb: lsb as usize,
    })
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamDecl {
    pub name: Symbol,
    pub dtype: Option<DataType>,
    pub range: Option<(Expr, Expr)>,
    pub default: Option<Expr>,
    pub is_localparam: bool,
    pub is_type_param: bool,
    pub type_default: Option<DataType>,
}

/// Override type param pada instansiasi (`#(.T(Wide16))`,
/// `#(.T(logic[15:0]))`): tipe + range packed literal bila ada.
/// `DataType` tidak membawa range, jadi range disimpan terpisah agar
/// lebar override literal ter-resolve (`logic[15:0]` → 16, bukan 1).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TypeParamAssign {
    pub dtype: DataType,
    pub range: Option<(Expr, Expr)>,
}

impl Default for TypeParamAssign {
    fn default() -> Self {
        TypeParamAssign {
            dtype: DataType::Void,
            range: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Decl {
    pub dtype: DataType,
    pub kind: DeclKind,
    pub names: Vec<DeclVar>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DeclKind {
    Wire,
    Wand,
    Wor,
    Tri,
    Tri0,
    Tri1,
    TriAnd,
    TriOr,
    Supply0,
    Supply1,
    Reg,
    Logic,
    Int,
    Integer,
}

impl DeclKind {
    pub fn is_net(&self) -> bool {
        matches!(
            self,
            DeclKind::Wire
                | DeclKind::Wand
                | DeclKind::Wor
                | DeclKind::Tri
                | DeclKind::Tri0
                | DeclKind::Tri1
                | DeclKind::TriAnd
                | DeclKind::TriOr
                | DeclKind::Supply0
                | DeclKind::Supply1
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeclVar {
    pub name: Symbol,
    pub range: Option<Range>,
    pub expr_range: Option<ExprRange>,
    pub array_range: Option<Range>,
    /// Ekspresi ukuran array unpacked `[N]` yang belum bisa di-resolve saat
    /// parse (mis. `[Width]` dengan parameter) — disimpan untuk di-resolve di
    /// elaborator bersama `effective_params`. Untuk `[N]` literal langsung
    /// di-resolve ke `array_range` ([N-1:0]) saat parse.
    pub array_size_expr: Option<Expr>,
    /// Dimensi unpacked LANJUTAN `[a][b]` setelah dimensi pertama
    /// (`array_range`/`array_size_expr`) — `logic [7:0] mat [0:1][0:1]`.
    /// Pasangan (range ter-resolve saat parse, ekspresi ukuran utk parameter).
    pub extra_unpacked_dims: Vec<(Option<Range>, Option<Expr>)>,
    pub extra_packed_dims: Vec<(ExprRange, Option<Range>)>,
    pub is_dynamic: bool,
    pub is_queue: bool,
    pub is_associative: bool,
    pub assoc_key_type: Option<DataType>,
    pub is_rand: bool,
    pub is_const: bool,
    /// Qualifier `static` pada variabel prosedural (function/task body).
    /// Variabel static menginisialisasi sekali dan mempertahankan nilainya
    /// antar pemanggilan function/task yang sama (IEEE 1800 §6.21).
    pub is_static: bool,
    pub expr: Option<Expr>,
}

impl DeclVar {
    pub fn resolved_width(&self, param_vals: &HashMap<Symbol, i64>) -> Result<usize, String> {
        let base_width = if let Some(r) = &self.range {
            r.width()
        } else if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            r.width()
        } else {
            1
        };
        // Multiply by all extra packed dim widths
        let mut total = base_width;
        for (er, _) in &self.extra_packed_dims {
            let r = resolve_expr_range(er, param_vals)?;
            total = total.saturating_mul(r.width());
        }
        Ok(total)
    }

    /// Returns all packed dimension widths from outermost to innermost.
    pub fn packed_dim_widths(
        &self,
        param_vals: &HashMap<Symbol, i64>,
    ) -> Result<Vec<usize>, String> {
        let first_width = if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            r.width()
        } else if let Some(r) = &self.range {
            r.width()
        } else {
            1usize
        };
        let mut dims = vec![first_width];
        for (er, _) in &self.extra_packed_dims {
            let r = resolve_expr_range(er, param_vals)?;
            dims.push(r.width());
        }
        Ok(dims)
    }

    /// Returns the width of the innermost element (last packed dim).
    pub fn innermost_width(&self, param_vals: &HashMap<Symbol, i64>) -> Result<usize, String> {
        if let Some((er, _)) = self.extra_packed_dims.last() {
            let r = resolve_expr_range(er, param_vals)?;
            Ok(r.width())
        } else if let Some(r) = &self.range {
            Ok(r.width())
        } else if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            Ok(r.width())
        } else {
            Ok(1)
        }
    }

    /// Returns the number of elements at the outermost packed dimension.
    pub fn outer_depth(&self, param_vals: &HashMap<Symbol, i64>) -> Result<usize, String> {
        if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            Ok(r.width())
        } else if let Some(r) = &self.range {
            Ok(r.width())
        } else {
            Ok(1)
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructMember {
    pub name: Symbol,
    pub dtype: Box<DataType>,
    pub range: Option<Range>,
    /// Range asli `[msb:lsb]` dalam bentuk ekspresi — disimpan bila bound
    /// memakai parameter/konstanta yang belum ter-resolve saat parse
    /// (mis. `logic [KeyLen-1:0] key` di package). Dipakai untuk menghitung
    /// width member saat `$bits(typedef)` / evaluasi konstanta.
    pub expr_range: Option<ExprRange>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DataType {
    Void,
    Bit,
    Logic,
    Int,
    Integer,
    Byte,
    Shortint,
    Longint,
    Time,
    Real,
    Realtime,
    String,
    Signed(Box<DataType>),
    UserDefined(Symbol),
    EnumType {
        base: Option<Box<DataType>>,
        members: Vec<(Symbol, Option<Expr>)>,
    },
    StructType {
        members: Vec<StructMember>,
    },
    UnionType {
        members: Vec<StructMember>,
    },
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::Void => write!(f, "void"),
            DataType::Bit => write!(f, "bit"),
            DataType::Logic => write!(f, "logic"),
            DataType::Int => write!(f, "int"),
            DataType::Integer => write!(f, "integer"),
            DataType::Byte => write!(f, "byte"),
            DataType::Shortint => write!(f, "shortint"),
            DataType::Longint => write!(f, "longint"),
            DataType::Time => write!(f, "time"),
            DataType::Real => write!(f, "real"),
            DataType::Realtime => write!(f, "realtime"),
            DataType::String => write!(f, "string"),
            DataType::Signed(inner) => write!(f, "signed {}", inner),
            DataType::UserDefined(name) => write!(f, "{}", name),
            DataType::EnumType { .. } => write!(f, "enum"),
            DataType::StructType { .. } => write!(f, "struct"),
            DataType::UnionType { .. } => write!(f, "union"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TypedefDecl {
    pub name: Symbol,
    pub dtype: DataType,
    pub range: Option<ExprRange>,
    /// Packed dimensions tambahan untuk tipe multidimensi
    /// (`typedef logic [4:0][4:0][W-1:0] box_t;`). Sebelumnya dibuang di
    /// parser sehingga width typedef salah (hanya range pertama dihitung) —
    /// sekarang disimpan agar `resolve_typedef_width` mengalikan semua dims.
    pub extra_packed_dims: Vec<ExprRange>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GateType {
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
    Buf,
    Not,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GatePrimitive {
    pub gate_type: GateType,
    pub instance_name: Option<Symbol>,
    pub ports: Vec<Expr>,
    pub drive_strength: Option<(String, String)>,
    pub delay: Option<Delay>,
}

/// Clocking event covergroup `@(posedge clk)` — edge disimpan penuh
/// (dulu dibuang parser → implicit sampling tak mungkin).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CovergroupClocking {
    /// true = `posedge`, false = `negedge` / tanpa edge (default posedge).
    pub posedge: bool,
    /// Signal event — `Expr` (biasanya `Expr::Ident`).
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CovergroupDecl {
    pub name: Symbol,
    /// Clocking event `@(posedge clk)` — utk IMPLICIT SAMPLING: sampel
    /// otomatis tiap edge (IEEE 1800 §19.8). Dulu `Option<Expr>` dan edge
    /// dibuang parser → tak pernah turun ke IR → covergroup `@event` tanpa
    /// `sample()` eksplisit = 0 samples senyap (gap sesi 2026-09-28).
    pub clocking_event: Option<CovergroupClocking>,
    pub coverpoints: Vec<CoverpointDef>,
    pub crosses: Vec<CrossDef>,
    /// VERIF-28: `type_option.weight = N` / `option.weight = N` — bobot
    /// covergroup utk perhitungan functional coverage keseluruhan (default 1).
    pub weight: Option<u64>,
    /// VERIF-28: `type_option.per_instance = 1` / `option.per_instance = 1` —
    /// coverage dilacak per-instance (default false = merge semua instance).
    pub per_instance: bool,
    /// Formal arguments `covergroup cg (int unsigned v, ...)` (IEEE 1800 §19.5).
    /// Dievaluasi saat `sample(a, b, ...)`; coverpoint yang merujuk formal
    /// hanya ter-sample bila binding sample() sudah didukung elaborasi.
    pub formals: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CoverpointDef {
    pub name: Symbol,
    pub expr: Expr,
    pub bins: Vec<BinDef>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CrossDef {
    pub name: Symbol,
    pub coverpoints: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BinDef {
    pub name: Symbol,
    pub range_list: Vec<BinRange>,
    pub bin_type: BinType,
    /// Transition bins `(a => b => ...)` — VERIF-31: tiap Vec<Expr> = satu
    /// sekuens nilai transisi (panjang 2 = kasus umum `prev => curr`).
    pub transitions: Vec<Vec<Expr>>,
}

/// Satu range nilai dalam bin: `[lo:hi]` (high Some) atau nilai tunggal
/// (high None). Representasi terpisah agar `{[1:5]}` (satu range) tidak
/// ambigu dengan `{1, 5}` (dua nilai) — sebelumnya dua expr didorong ke
/// Vec<Expr> tanpa penanda range (VERIF-30).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BinRange {
    pub low: Expr,
    pub high: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BinType {
    Normal,
    Illegal,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DpiImport {
    pub name: Symbol,
    pub return_type: Option<Box<DataType>>,
    pub args: Vec<DpiArg>,
    pub is_task: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DpiArg {
    pub direction: PortDirection,
    pub dtype: DataType,
    pub name: Symbol,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ModuleItem {
    Always(AlwaysBlock),
    Initial(InitialBlock),
    Final(InitialBlock),
    Assign(ContinuousAssign),
    Instance(ModuleInstance),
    Gate(GatePrimitive),
    Decl(Decl),
    Func(FunctionDecl),
    Generate(GenerateBlock),
    Typedef(TypedefDecl),
    Covergroup(CovergroupDecl),
    // Imported items from packages
    Import {
        package: Symbol,
        item: Symbol,
    },
    DpiImport(DpiImport),
    DpiExport(DpiImport),
    Param(ParamDecl),
    Clocking(ClockingBlock),
    Specify(SpecifyBlock),
    VirtualInterface {
        iface_type: Symbol,
        modport: Option<Symbol>,
        vif_name: Symbol,
    },
    /// LANG-40: `let name = expr;` / `let name(a, b) = expr;` — deklarasi
    /// let (IEEE 1800-2017 §11.12.2): alias ekspresi scoped. Parameter
    /// kosong = konstanta ekspresi; parameter di-substitusi saat pemakaian.
    Let(LetDecl),
    /// LANG-08: `alias a = b, c;` — net alias network (IEEE 1800-2017 §10.9):
    /// semua net dalam satu deklarasi alias menjadi satu jaringan — menulis
    /// ke salah satu akan terlihat di semua (short). List pasangan (lhs, rhs).
    NetAlias(Vec<(Expr, Expr)>),
    /// LANG-08: `nettype <type> <name>;` — user-defined net type (IEEE
    /// 1800-2017 §6.10): mendaftarkan tipe net baru yang mengikat ke tipe
    /// data dasar; deklarasi variabel dgn tipe ini di-resolve ke base type.
    Nettype(NettypeDecl),
    /// Class declaration (IEEE 1800-2017 §8)
    Class(ClassDecl),
    /// LANG-04/11/12/13: concurrent assertion property module-level —
    /// `assert/assume/cover/restrict property (@(posedge clk) expr)` bentuk
    /// BOOLEAN (tanpa operator temporal `##`/`|->`/`[*]`). Disimpan sebagai
    /// Stmt::Assert/Assume/Cover dgn clock_event + disable_iff; elaborator
    /// mengubahnya jadi AlwaysBlock ber-clock sehingga dievaluasi tiap edge.
    /// Property dgn operator temporal tetap di-skip parser (limitation).
    PropertyAssert(Box<Stmt>),
    /// LANG-10: `checker name (ports); items endchecker` (IEEE 1800-2017
    /// §17.8) — unit assertion terinstansiasi. Saat checker diinstansiasi di
    /// module, assertion property di `items` di-drive dengan port binding.
    Checker(CheckerDecl),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CheckerDecl {
    /// Nama checker.
    pub name: Symbol,
    /// Nama port (urut positional).
    pub ports: Vec<Symbol>,
    /// Body items — assertion/property/let/dll di dalam checker.
    pub items: Vec<ModuleItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LetDecl {
    pub name: Symbol,
    /// Parameter formal let (bisa kosong).
    pub params: Vec<Symbol>,
    /// Ekspresi body — dievaluasi saat pemakaian `name(args)` dengan
    /// parameter disubstitusi oleh argumen.
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NettypeDecl {
    /// Nama tipe net user (`nettype logic [7:0] mynet;` → "mynet").
    pub name: Symbol,
    /// Tipe data dasar (logic/bit/wire + range) yang di-alias.
    pub base: DataType,
    /// Range packed `[msb:lsb]` (mis. `nettype logic [7:0] mynet;`).
    pub range: Option<ExprRange>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GenerateBlock {
    pub items: Vec<GenerateItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum GenerateItem {
    If {
        cond: Expr,
        true_items: Vec<ModuleItem>,
        false_items: Vec<ModuleItem>,
        /// Label blok `begin : name` pada branch true (untuk scope hierarki).
        label: Option<Symbol>,
    },
    For {
        var: Symbol,
        init: Option<Stmt>,
        cond: Option<Expr>,
        step: Option<Stmt>,
        body_items: Vec<ModuleItem>,
        /// Label blok `begin : name` pada badan loop. Dipakai untuk menamai
        /// sinyal lokal per iterasi (`name[k].sig`) agar tidak collide.
        label: Option<Symbol>,
    },
    Case {
        case_type: GenerateCaseType,
        expr: Expr,
        items: Vec<CaseGenerateItem>,
        default: Option<Vec<ModuleItem>>,
    },
    Items(Vec<ModuleItem>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GenerateCaseType {
    Normal,
    CaseX,
    CaseZ,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CaseGenerateItem {
    pub labels: Vec<Expr>,
    pub body: Vec<ModuleItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDecl {
    pub name: Symbol,
    pub range: Option<ExprRange>,
    pub return_type: Option<Box<DataType>>,
    pub ports: Vec<FunctionPort>,
    pub decls: Vec<Decl>,
    pub stmts: Vec<Stmt>,
    pub virtual_flag: bool,
    pub is_static: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PackageDecl {
    pub name: Symbol,
    pub items: Vec<PackageItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PackageItem {
    Decl(Decl),
    Function(FunctionDecl),
    Task(TaskDecl),
    Typedef(TypedefDecl),
    Param(ParamDecl),
    Class(ClassDecl),
    Import { package: Symbol, item: Symbol },
    Export { package: Symbol, item: Symbol },
    DpiImport(DpiImport),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClockingBlock {
    pub name: Symbol,
    pub clock_event: ClockEvent,
    pub default_input_skew: Option<u64>,
    pub default_output_skew: Option<u64>,
    pub items: Vec<ClockingItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClockEvent {
    Posedge(Symbol),
    Negedge(Symbol),
    Edge(Symbol),
}

/// SVA temporal sequence (IEEE 1800-2017 §16) — representasi AST dari
/// properti ber-urutan. Dievaluasi ber-clock oleh engine via
/// `IrSequence` (elaborator menerjemahkan enum ini). Bentuk yang didukung:
/// boolean `Expr`, delay `##N`/`##[min:max]`, `Concat` (seq1 ##1 seq2),
/// `Or`/`And`, dan `Repeat` (`[*N]`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Sequence {
    /// Immediate Boolean expression (dievaluasi tiap cycle)
    Expr(Expr),
    /// ##N — tunggu N clock cycles
    Delay(u64),
    /// ##[min:max] — tunggu antara min..max clock cycles
    DelayRange(u64, u64),
    /// seq1 ##1 seq2 — concatenation (dulu, lalu kedua)
    Concat(Box<Sequence>, Box<Sequence>),
    /// seq1 or seq2 — salah satu cocok
    Or(Box<Sequence>, Box<Sequence>),
    /// seq1 and seq2 — keduanya harus cocok
    And(Box<Sequence>, Box<Sequence>),
    /// seq[*N] — ulangi seq N kali berturut-turut
    Repeat(Box<Sequence>, u64),
    /// ante |-> cons — overlap implication (IEEE 1800-2017 §16.9.2):
    /// saat antecedent match, consequent dievaluasi mulai cycle YANG SAMA.
    Implication(Box<Sequence>, Box<Sequence>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClockingItem {
    Input {
        signals: Vec<Symbol>,
        skew: Option<u64>,
    },
    Output {
        signals: Vec<Symbol>,
        skew: Option<u64>,
    },
    InputOutput {
        signals: Vec<Symbol>,
    },
    DefaultInputSkew(u64),
    DefaultOutputSkew(u64),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionPort {
    pub name: Symbol,
    pub range: Option<Range>,
    pub expr_range: Option<ExprRange>,
    pub direction: Option<PortDirection>,
    /// Default value (`task f(int x = 5)`) — dipakai inline untuk port yang
    /// TIDAK di-pass saat call (formal harus tetap di-rename agar body tidak
    /// meninggalkan nama formal → E2001). None bila tanpa default.
    pub default: Option<Expr>,
}

impl FunctionPort {
    pub fn resolved_width(&self, param_vals: &HashMap<Symbol, i64>) -> Result<usize, String> {
        if let Some(r) = &self.range {
            Ok(r.width())
        } else if let Some(er) = &self.expr_range {
            let r = resolve_expr_range(er, param_vals)?;
            Ok(r.width())
        } else {
            Ok(1)
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UdpPort {
    pub direction: PortDirection,
    pub name: Symbol,
    pub is_reg: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UdpSymbol {
    Zero,
    One,
    X,
    DontCare,
    Edge(Symbol),
    NoChange,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UdpTableEntry {
    pub inputs: Vec<UdpSymbol>,
    pub output: UdpSymbol,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UdpDef {
    pub name: Symbol,
    pub ports: Vec<UdpPort>,
    pub table: Vec<UdpTableEntry>,
    pub is_sequential: bool,
    pub initial_output: Option<UdpSymbol>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContinuousAssign {
    pub lhs: Expr,
    pub rhs: Expr,
    pub delay: Option<Delay>,
}

/// Satu pasangan atribut Verilog `(* key = value *)` (IEEE 1800 §22.17),
/// disederhanakan jadi `key` + nilai teks opsional (string/angka/ident,
/// atau tanpa nilai). Dipakai anotasi emulator (EMULATOR.md §10):
/// `(* mivon_region = "mmio", base = "0x10000000", size = "0x1000" *)`,
/// `(* mivon_irq = "5" *)` — lihat `mivon-emu::mhir`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AttrEntry {
    pub key: Symbol,
    pub value: Option<String>,
}

impl AttrEntry {
    /// Cari nilai atribut `key` (case-sensitive), None bila tak ada/tanpa nilai.
    pub fn value_of<'a>(attrs: &'a [AttrEntry], key: &str) -> Option<&'a str> {
        attrs
            .iter()
            .find(|a| a.key.as_str() == key)
            .and_then(|a| a.value.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModuleInstance {
    pub module_name: Symbol,
    pub instance_name: Symbol,
    pub range: Option<ExprRange>,
    pub param_assigns: HashMap<Symbol, Expr>,
    pub type_param_assigns: HashMap<Symbol, TypeParamAssign>,
    pub port_conns: Vec<PortConnection>,
    /// Posisi token module name di source (untuk diagnostic).
    pub line: usize,
    pub col: usize,
    /// Atribut `(* ... *)` sebelum instance (urut sumber); kosong bila tak
    /// ada. HARUS field terakhir — cache bincode (MICD) yang lama gagal
    /// decode (EOF) → rebuild, bukan data salah baca.
    pub attrs: Vec<AttrEntry>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PortConnection {
    Positional(Expr),
    Named {
        port: Symbol,
        expr: Expr,
    },
    /// Koneksi port KOSONG (`.data_o()`) = TIDAK terhubung (LRM 1800 §23.3.2.6).
    /// Input yang tak terhubung mengambang (Z/X); output yang tak terhubung
    /// hanya didorong sisi child. JANGAN diubah jadi literal 0 (konflik
    /// multiple-driver → osilasi, terlihat di `prim_secded` checkers OpenTitan).
    Unconnected {
        port: Symbol,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Delay {
    pub rise: Option<Expr>,
    pub fall: Option<Expr>,
    pub turnoff: Option<Expr>,
}

/// Arah edge pada timing-check reference event (SIM-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeKind {
    PosEdge,
    NegEdge,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SpecifyItem {
    PathDelay {
        src: Symbol,
        dst: Symbol,
        rise: Option<Expr>,
        fall: Option<Expr>,
    },
    SpecParam {
        name: Symbol,
        value: Expr,
    },
    SetupCheck {
        data: Expr,
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        limit: Expr,
    },
    HoldCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        limit: Expr,
    },
    SetupHoldCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        setup_limit: Expr,
        hold_limit: Expr,
    },
    RecoveryCheck {
        data: Expr,
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        limit: Expr,
    },
    RemovalCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        limit: Expr,
    },
    RecoveryRemovalCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        recovery_limit: Expr,
        removal_limit: Expr,
    },
    PeriodCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        limit: Expr,
    },
    WidthCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        limit: Expr,
        threshold: Option<Expr>,
    },
    NochangeCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        start_limit: Expr,
        end_limit: Expr,
    },
    SkewCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        limit: Expr,
    },
    TimeskewCheck {
        ref_event: Expr,
        ref_edge: Option<EdgeKind>,
        data: Expr,
        limit: Expr,
        threshold: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpecifyBlock {
    pub items: Vec<SpecifyItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum VarType {
    Reg,
    Logic,
    Int,
    Integer,
}
