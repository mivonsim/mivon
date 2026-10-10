//! Mivon HDL (.mv) — AST (Abstract Syntax Tree).
//! Bahasa baru milik Mivon yang di-transpile ke SystemVerilog (.sv/.svh).
//! Lihat MIVON-HDL.md untuk spesifikasi lengkap.
//!
//! 1 file = 1 tanggung jawab: hanya definisi tipe AST, tanpa parsing/codegen.

/// Arah port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
    Inout,
}

/// Tipe data Mivon HDL.
#[derive(Debug, Clone, PartialEq)]
pub enum MvType {
    /// `bit` — 1-bit 2-state
    Bit,
    /// `logic` / `logic[N]` / `logic[msb:lsb]`
    Logic(Option<(Expr, Expr)>),
    /// `signed logic[N]`
    Signed(Box<MvType>),
    Int,
    Uint,
    LongInt,
    ULongInt,
    ShortInt,
    Byte,
    Real,
    Time,
    Str,
    /// Tipe user-defined: `State`, `Packet`, `Addr`, ... Posisi (line, col)
    /// untuk E2005 tipe tak dikenal (F11).
    Named(String, usize, usize),
    /// Array unpacked `Type[N][M]`
    Array(Box<MvType>, Vec<Expr>),
    /// Queue SV `Type[$]` — dynamic array (push_back/pop_front/size/delete).
    /// F40: `sig q : logic[7:0][$]` → SV `logic [7:0] q[$];`.
    Queue(Box<MvType>),
}

/// Ekspresi Mivon HDL.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Desimal `123`
    Int(i64),
    /// Literal bertipe `8'hFF`, `'b101` (width opsional) — posisi (line, col)
    /// untuk E2006 (F11).
    Sized(Option<i64>, char, String, usize, usize),
    /// `1.5`
    Real(f64),
    /// Fill literal `'0` `'1` `'x` `'z`
    Fill(char),
    /// String literal
    Str(String),
    /// Ident `count` — membawa posisi (line, col) untuk E2001 (F11).
    Ident(String, usize, usize),
    /// Scoped `pkg::item` — posisi (line, col) untuk E2001 (F11).
    Scoped(String, String, usize, usize),
    /// F33: type cast `T'(expr)` / `Word16'(x)` — tipe target (bisa type
    /// param `T`, typedef, tipe dasar, scoped `pkg::T`). Posisi (line, col)
    /// untuk error validasi.
    Cast {
        // Box<MvType> — MvType::Logic menyimpan Expr tanpa Box, jadi perlu
        // indirection utk memutus rekursi Expr ↔ MvType (E0072).
        ty: Box<MvType>,
        expr: Box<Expr>,
        line: usize,
        col: usize,
    },
    /// Unary `-x`, `~x`, `!x`, `&x`, `|x`, `^x`
    Unary(String, Box<Expr>),
    /// F37: `++x` / `x++` / `--x` / `x--` di level EKSPRESI (RHS).
    /// inc = true utk `++`, false utk `--`; pre = true utk prefix `++x`,
    /// false utk postfix `x++`.
    IncDec {
        inc: bool,
        pre: bool,
        expr: Box<Expr>,
    },
    /// Binary `a + b`, `a && b`, `a == b`, `a <= b` ...
    Binary(String, Box<Expr>, Box<Expr>),
    /// Ternary `c ? a : b`
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Call `$display(...)`, `clog2(...)`, `pkg::func(...)`.
    /// `(line, col)` = posisi nama fungsi — dipakai E2011 (jumlah argumen).
    Call(String, Vec<Expr>, usize, usize),
    /// Named argument dalam call: `f(10, factor = 4)` — nama dengan `=`,
    /// di-emit SV `.factor(4)`. Hanya valid di argumen call.
    NamedArg { name: String, expr: Box<Expr> },
    /// Method call `obj.method(args)` (obj bisa `this`/`super`/var)
    MethodCall {
        obj: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        /// posisi nama method — E2011
        line: usize,
        col: usize,
    },
    /// Member `packet.valid` — posisi (line, col) untuk E2001 field (F11).
    Member(Box<Expr>, String, usize, usize),
    /// Index `q[i]`
    Index(Box<Expr>, Box<Expr>),
    /// Range select `q[i-1:0]`
    Range(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Indexed part-select `q[i +: 8]` (naik) / `q[i -: 8]` (turun).
    /// `width` adalah LEBAR bit, bukan indeks akhir — LRM 1800 §11.8.2.
    PartSelect {
        base: Box<Expr>,
        from: Box<Expr>,
        width: Box<Expr>,
        /// true = `+:` (naik), false = `-:` (turun)
        plus: bool,
    },
    /// Concatenation `{a, b}`
    Concat(Vec<Expr>),
    /// Array literal `'{e0, e1, ...}` (assignment pattern) — unpacked array
    /// initializer/deklarasi ROM. Di-emit `'{...}` (bukan concat `{...}`).
    ArrayLit(Vec<Expr>),
    /// Replication `{n{a}}`
    Replicate(Box<Expr>, Box<Expr>),
    /// Paren `(a + b)`
    Paren(Box<Expr>),
    /// `x inside {a, b, [lo:hi]}` — constraint membership, urutan item dijaga (F12).
    Inside {
        expr: Box<Expr>,
        items: Vec<InsideItem>,
    },
    /// `x dist { 0 := 10, [1:5] :/ 20 }` — constraint distribusi (F12).
    Dist {
        expr: Box<Expr>,
        items: Vec<DistItem>,
    },
}

/// Satu anggota himpunan `inside`: nilai tunggal atau rentang `[lo:hi]` (F12).
#[derive(Debug, Clone, PartialEq)]
pub enum InsideItem {
    Value(Expr),
    Range(Expr, Expr),
}

/// Item `dist`: nilai tunggal atau range `[lo:hi]` dengan bobot (F12).
/// `exact = true` → `:=` (tiap nilai dapat bobot penuh); `false` → `:/`
/// (bobot dibagi rata antar nilai dalam range).
#[derive(Debug, Clone, PartialEq)]
pub struct DistItem {
    /// Nilai tunggal (dipakai saat `range` None).
    pub value: Expr,
    /// Range `[lo:hi]` (opsional).
    pub range: Option<(Expr, Expr)>,
    pub weight: Expr,
    pub exact: bool,
}

/// Statement Mivon HDL.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `{ stmt* }`
    Block(Vec<Stmt>),
    /// F47: `label : { stmt* }` — blok bernama (emit `begin : label ... end`).
    /// Dipakai target `disable <label>` (F45) — label harus unik per scope.
    NamedBlock {
        name: String,
        stmts: Vec<Stmt>,
    },
    /// `lhs = rhs` (blocking) / `lhs <= rhs` (non-blocking).
    /// Posisi (line, col) statement untuk E2002/E2003/E2004 (F11).
    Assign {
        lhs: Expr,
        rhs: Expr,
        nba: bool,
        line: usize,
        col: usize,
    },
    /// F36: `lhs += rhs` — compound assignment (blocking). op salah satu dari
    /// "+=" "-=" "*=" "/=" "%=" "<<=" ">>=" "&=" "|=" "^="".
    CompoundAssign {
        lhs: Expr,
        op: String,
        rhs: Expr,
        line: usize,
        col: usize,
    },
    /// F36/F37: `lhs++` / `++lhs` — increment/decrement (blocking).
    /// inc = true utk `++`, false utk `--`; pre = true utk prefix `++lhs`
    /// (F37), false utk postfix `lhs++` (F36). Hasil statement-level identik.
    IncDec {
        lhs: Expr,
        inc: bool,
        pre: bool,
        line: usize,
        col: usize,
    },
    /// `if (cond) then else` — `qual` = qualifier `unique`/`unique0`/
    /// `priority` (None utk `if` biasa). Qualifier hanya di head rantai;
    /// `else if` lanjutan selalu plain (LRM 1800 §12.5).
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
        qual: Option<String>,
    },
    /// `case (expr) { items... default: ... }` (F26: qualifier
    /// priority/unique/unique0 + kind casez/casex — `qual` None utk `case`
    /// biasa, `kind` "case"/"casez"/"casex").
    Case {
        expr: Expr,
        items: Vec<(Vec<Expr>, Stmt)>,
        default: Option<Box<Stmt>>,
        qual: Option<String>,
        kind: String,
        /// posisi keyword `case` — E2013 (label duplikat)
        line: usize,
        col: usize,
    },
    /// `case (expr) inside { v: ...; [lo:hi]: ...; default: ... }` —
    /// membership + rentang (LRM 1800 §12.5). Label `InsideItem::Value`
    /// atau `Range` (bukan `Expr` biasa — `[lo:hi]` bukan select di sini).
    CaseInside {
        expr: Expr,
        items: Vec<(Vec<InsideItem>, Stmt)>,
        default: Option<Box<Stmt>>,
        qual: Option<String>,
        /// posisi keyword `case` — E2013 (label duplikat)
        line: usize,
        col: usize,
    },
    /// `for var in from..to { body }` — optional `step` : `for i in 0..N step 2`
    /// → increment `i = i + 2` di SV (default `+ 1`).
    For {
        var: String,
        from: Expr,
        to: Expr,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    /// `while (cond) { body }`
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// F38: `do { body } while (cond)` — loop post-test (body dijalankan
    /// minimal sekali).
    DoWhile {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// F38: event trigger `->ev` — memicu event named (emisi `-> ev;`).
    EventTrigger(Expr),
    /// `repeat (count) { body }`
    Repeat {
        count: Expr,
        body: Box<Stmt>,
    },
    /// `forever { body }`
    Forever(Box<Stmt>),
    /// `wait (cond) { body }`
    Wait {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// F45: `wait fork;` — tunggu semua child fork milik proses ini selesai.
    WaitFork,
    /// F45: `disable fork;` / `disable <label>;` — terminasi child fork / blok bernama.
    Disable {
        name: String,
    },
    /// F46: `force x = expr;` — override paksa sinyal (fault injection TB).
    /// Blocking, seperti `=`: hanya di luar `seq`.
    Force {
        lhs: Expr,
        rhs: Expr,
        line: usize,
        col: usize,
    },
    /// F46: `release x;` — lepas force, sinyal kembali ke driver.
    Release {
        target: Expr,
    },
    /// `@(event) body` — body opsional (untuk `repeat (n) @(posedge clk)`).
    Event {
        expr: Expr,
        body: Option<Box<Stmt>>,
    },
    /// `#amt body`
    Delay {
        amt: Expr,
        body: Box<Stmt>,
    },
    /// Expression statement `$display(...)`
    ExprStmt(Expr),
    /// `var r : int = 0` — deklarasi variabel lokal (di dalam func/task)
    VarDecl {
        names: Vec<String>,
        ty: MvType,
        init: Option<Expr>,
        line: usize,
        col: usize,
    },
    /// `return expr`
    Return(Option<Expr>, usize, usize),
    Break(usize, usize),
    Continue(usize, usize),
    /// F39: `fork { ... } join / join_any / join_none` — branch berjalan
    /// konkurren (masing-masing independen, delay sendiri).
    Fork {
        branches: Vec<Stmt>,
        join: ForkJoin,
    },
    /// `foreach (arr[i]) { body }` / `foreach (mat[i][j]) ...` — loop per
    /// elemen array unpacked (index var otomatis bertipe int).
    Foreach {
        arr: String,
        inds: Vec<String>,
        body: Box<Stmt>,
    },
    /// `assert (cond) pass [else fail]`
    Assert {
        cond: Expr,
        pass: Option<Box<Stmt>>,
        fail: Option<Box<Stmt>>,
    },
    /// `assert property (...)` — concurrent assertion.
    /// Body dipertahankan RAW (teks persis di antara `(` dan `)`) karena
    /// berisi operator SVA (`|->`, `##`) yang bukan token `.mv` — emisi 1:1.
    AssertProperty(String),
    /// `assume (cond) pass [else fail]` — asumsi formal (mirror `assert`,
    /// LRM 1800 §20.11: branch pass diakhiri `;` sebelum `else`).
    Assume {
        cond: Expr,
        pass: Option<Box<Stmt>>,
        fail: Option<Box<Stmt>>,
    },
    /// `assume property (...)` — asumsi concurrent (module item, LRM 1800 §14).
    /// Body RAW seperti `AssertProperty`.
    AssumeProperty(String),
    /// `cover (cond) [pass]` — titik coverage (tanpa `else`: tak ada cabang
    /// gagal; bila `cond` benar, `pass` jalan + hit dicatat engine).
    Cover {
        cond: Expr,
        pass: Option<Box<Stmt>>,
    },
    /// `cover property (...)` — cover concurrent (module item, LRM 1800 §14).
    /// Body RAW seperti `AssertProperty`.
    CoverProperty(String),
    /// Escape hatch: `@sv { ... }` — teks SystemVerilog mentah yang di-emit
    /// verbatim (untuk konstruk SV yang belum didukung bahasa). Body diambil
    /// mentah dari source oleh lexer (bukan token .mv).
    RawSvh(String),
}

/// F39: mode sinkronisasi fork/join (MIVON-HDL.md §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkJoin {
    /// `join` — tunggu SEMUA branch selesai
    Join,
    /// `join_any` — lanjut saat branch PERTAMA selesai
    JoinAny,
    /// `join_none` — lanjut segera, branch berjalan di background
    JoinNone,
}

/// F76: varian net Verilog (LRM 1800 §6.5) — resolution multi-driver.
/// `triand`/`trior` = alias `wand`/`wor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKind {
    Wire,
    Wand,
    Wor,
    Tri,
    Tri0,
    Tri1,
    Supply0,
    Supply1,
}

impl NetKind {
    /// Nama SV untuk emisi (`wand`, `wor`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            NetKind::Wire => "wire",
            NetKind::Wand => "wand",
            NetKind::Wor => "wor",
            NetKind::Tri => "tri",
            NetKind::Tri0 => "tri0",
            NetKind::Tri1 => "tri1",
            NetKind::Supply0 => "supply0",
            NetKind::Supply1 => "supply1",
        }
    }
}

/// Field struct/union: `valid : bit, addr : Addr`. Posisi (line, col) untuk
/// E2007 duplikat field (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub names: Vec<String>,
    pub ty: MvType,
    pub line: usize,
    pub col: usize,
}

/// Member enum: `RED = 0`. Posisi (line, col) untuk E2007 duplikat member (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct EnumMember {
    pub name: String,
    pub value: Option<Expr>,
    pub line: usize,
    pub col: usize,
}

/// Typedef level file/package. Posisi (line, col) nama untuk E2007/E2005 (F11).
#[derive(Debug, Clone, PartialEq)]
pub enum Typedef {
    /// `type Addr = logic[15:0]`
    Alias {
        name: String,
        ty: MvType,
        line: usize,
        col: usize,
    },
    /// `packed struct Packet { ... }`
    Struct {
        name: String,
        packed: bool,
        fields: Vec<Field>,
        line: usize,
        col: usize,
    },
    /// `packed union Word { ... }`
    Union {
        name: String,
        packed: bool,
        fields: Vec<Field>,
        line: usize,
        col: usize,
    },
    /// `enum State { IDLE, RUN }` / `enum(3) Color { ... }`
    Enum {
        name: String,
        width: Option<Expr>,
        members: Vec<EnumMember>,
        line: usize,
        col: usize,
    },
}

/// Port module: `in clk, rst_n : bit`. Posisi (line, col) untuk E2007 (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct Port {
    pub dir: Dir,
    pub names: Vec<String>,
    pub ty: MvType,
    pub line: usize,
    pub col: usize,
}

/// Parameter module: `WIDTH : int = 8` / `T : type = logic[8]`. Posisi untuk
/// E2007 duplikat parameter (F11).
///
/// Parameter module (F31/F32):
/// - nilai: `WIDTH : int = 8` → `ty = Some(Int)`, `default = Some(8)`
/// - type:  `T : type = logic[7:0]` / `type T = logic[7:0]` →
///   `ty = Some(Named("type"))` (marker), `type_default = Some(logic[7:0])`
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<MvType>,
    pub default: Option<Expr>,
    /// F32: default TIPE utk type parameter (`type T = logic[7:0]`).
    /// Terisi hanya saat `ty` adalah marker `Named("type")`.
    pub type_default: Option<MvType>,
    pub line: usize,
    pub col: usize,
}

/// Spesifikasi `seq(clk, rst_n)`. Posisi (line, col) untuk E2001 clock/reset
/// tak dikenal (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct SeqSpec {
    pub clk: String,
    pub neg_edge: bool,
    /// (nama reset, active_low, sync)
    pub reset: Option<(String, bool, bool)>,
    pub line: usize,
    pub col: usize,
}

/// Koneksi port instansiasi.
#[derive(Debug, Clone, PartialEq)]
pub enum Conn {
    /// `.port(expr)` atau `.port` (auto-connect)
    Named { port: String, expr: Option<Expr> },
    /// Positional `(a, b)`
    Positional(Expr),
}

/// Function `.mv`: `func clog2(x : int) -> int { ... }`. Posisi (line, col)
/// nama untuk E2007 duplikat function (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct MFunc {
    pub name: String,
    /// (nama, tipe, arah opsional, default arg opsional `b : int = 4`)
    pub args: Vec<(String, MvType, Option<Dir>, Option<Expr>)>,
    pub ret: Option<MvType>,
    pub body: Vec<Stmt>,
    pub line: usize,
    pub col: usize,
}

/// Task `.mv`: `task send(data : logic[7:0], out ok : bit) { ... }`. Posisi
/// (line, col) nama untuk E2007 duplikat task (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct MTask {
    pub name: String,
    /// (nama, tipe, arah opsional, default arg opsional)
    pub args: Vec<(String, MvType, Option<Dir>, Option<Expr>)>,
    pub body: Vec<Stmt>,
    pub line: usize,
    pub col: usize,
}

/// Item dalam blok `constraint c { ... }` (F12 — hapus batasan subset F7):
/// ekspresi relasional/equality (termasuk `inside`/`dist`), `if/else`, dan
/// `solve x before y`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintItem {
    /// Ekspresi relasional/equality/inside/dist — `seed > 10`, `x inside {...}`.
    Expr(Expr),
    /// `if (cond) { items } else { items }` — hanya cabang yang terpenuhi
    /// yang diterapkan (di-emit 1:1, dievaluasi kondisional oleh solver SV).
    If {
        cond: Expr,
        then: Vec<ConstraintItem>,
        els: Vec<ConstraintItem>,
    },
    /// `solve var before a, b` — urutan solusi (emisi 1:1). Posisi (line,
    /// col) `solve` untuk E2001 var tak dikenal (F11/F12).
    Solve {
        var: String,
        before: Vec<String>,
        line: usize,
        col: usize,
    },
}

/// Class `.mv` (MIVON-HDL.md §8): `class my_test extends uvm_test { ... }`.
/// Posisi (line, col) nama untuk E2007 duplikat class (F11).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MClass {
    pub name: String,
    /// `extends Base` (opsional)
    pub extends: Option<String>,
    /// (nama, tipe, rand) — `field count : uint` / `rand field seed : uint`
    pub fields: Vec<(String, MvType, bool)>,
    /// (nama, item constraint) — `constraint c { seed > 10, seed < 200 }`
    pub constraints: Vec<(String, Vec<ConstraintItem>)>,
    pub funcs: Vec<MFunc>,
    pub tasks: Vec<MTask>,
    pub line: usize,
    pub col: usize,
}

/// Item dalam module (badan `module { }`).
#[derive(Debug, Clone, PartialEq)]
pub enum MItem {
    Port(Port),
    /// `typedef` lokal module — `type X = ...` / `packed struct S {...}` /
    /// `enum E {...}` di dalam badan module (tipe scope-lokal, SV valid).
    Typedef(Typedef),
    /// `sig x : logic[7:0]` — posisi (line, col) untuk E2007/E2005 (F11).
    Sig {
        names: Vec<String>,
        ty: MvType,
        init: Option<Expr>,
        line: usize,
        col: usize,
    },
    /// `reg x : logic[7:0] = '0` — posisi (line, col) untuk E2007/E2005 (F11).
    Reg {
        names: Vec<String>,
        ty: MvType,
        init: Option<Expr>,
        line: usize,
        col: usize,
    },
    /// F75: `wire w : logic[7:0]` — net Verilog (LRM 1800 §6.5/§10.2).
    /// Di-emit `wire ...;`, bukan `logic`. Init opsional di-emit inline
    /// (`wire [7:0] w = expr;` — net decl assignment, sah §10.2).
    /// F76: `net` = varian resolution (`wand`/`wor`/`tri`/...) — LRM §6.5.
    Wire {
        net: NetKind,
        names: Vec<String>,
        ty: MvType,
        init: Option<Expr>,
        line: usize,
        col: usize,
    },
    /// F75: `assign y = expr` — continuous assignment level module
    /// (LRM 1800 §10.2). Di-emit `assign y = expr;`.
    Assign {
        lhs: Expr,
        rhs: Expr,
        line: usize,
        col: usize,
    },
    /// `const NAME = expr` — posisi (line, col) untuk E2007 (F11).
    Const {
        name: String,
        ty: Option<MvType>,
        value: Expr,
        line: usize,
        col: usize,
    },
    /// `use pkg::*` / `use pkg::item`
    Use {
        pkg: String,
        item: String,
    },
    /// `seq(clk, rst_n) { ... }` → always_ff
    Seq(SeqSpec, Stmt),
    /// `comb { ... }` → always_comb
    Comb(Stmt),
    /// `always { ... }`
    Always(Stmt),
    /// `latch { ... }` → always_latch
    Latch(Stmt),
    /// `initial { ... }`
    Initial(Stmt),
    /// `final { ... }`
    Final(Stmt),
    /// `inst mod_name inst_name (...)` / `inst mod_name inst_name[8] (...)`
    /// Posisi (line, col) nama module utk E2001/E2005 saat validasi koneksi
    /// port (F29).
    Inst {
        module: String,
        name: String,
        dims: Option<Expr>,
        params: Vec<(String, Expr)>,
        conns: Vec<Conn>,
        line: usize,
        col: usize,
    },
    /// F78: `bind target mod_name inst_name (...)` — ikat instance checker ke
    /// scope target hierarkis (LRM 1800 §23.11, 1:1 ke SV `bind`). `target`
    /// path dotted (`dut`, `top.u_mem`). Posisi (line, col) nama module.
    Bind {
        target: String,
        module: String,
        name: String,
        dims: Option<Expr>,
        params: Vec<(String, Expr)>,
        conns: Vec<Conn>,
        line: usize,
        col: usize,
    },
    /// `for i in 1..N { ... }` (generate) — optional `step` : `for i in 0..8 step 2`
    GenFor {
        var: String,
        from: Expr,
        to: Expr,
        step: Option<Expr>,
        body: Vec<MItem>,
    },
    /// `if (cond) { ... } else { ... }` (generate)
    GenIf {
        cond: Expr,
        then: Vec<MItem>,
        els: Vec<MItem>,
    },
    Func(MFunc),
    Task(MTask),
    /// `assert property ( … )` di LEVEL MODULE — concurrent assertion.
    ///
    /// LRM 1800 §14: `assert property` bersifat CONCURRENT, ia module item
    /// (atau *concurrent assertion item*), BUKAN statement prosedural.
    /// Menuliskannya di dalam `initial`/`always` menghasilkan SV yang
    /// ditolak tool ("Procedural concurrent assertion … inside always",
    /// IEEE 1800-2017 §16.14.6). Body disimpan raw karena berisi operator
    /// SVA yang bukan token `.mv`.
    AssertProperty(String),
    /// `assume property ( … )` di LEVEL MODULE — asumsi concurrent
    /// (module item, LRM 1800 §14). Mirror `AssertProperty`.
    AssumeProperty(String),
    /// `cover property ( … )` di LEVEL MODULE — cover concurrent
    /// (module item, LRM 1800 §14). Mirror `AssertProperty`.
    CoverProperty(String),
}

/// Module `.mv`. Posisi (line, col) nama untuk E2007 duplikat (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub name: String,
    pub params: Vec<Param>,
    pub items: Vec<MItem>,
    pub line: usize,
    pub col: usize,
}

/// Modport interface: `modport slave { in a, b; out c }` (MIVON-HDL.md §6.10).
/// `dirs` = urutan deklarasi arah + daftar nama signal. Posisi (line, col)
/// nama untuk E2007 duplikat modport (F26).
#[derive(Debug, Clone, PartialEq)]
pub struct Modport {
    pub name: String,
    pub dirs: Vec<(Dir, Vec<String>)>,
    pub line: usize,
    pub col: usize,
}

/// Interface `.mv`: `interface axi_lite { ... }` (MIVON-HDL.md §6.10).
/// Body berisi port (`in clk : bit` — di-emit sebagai deklarasi signal
/// interface), `sig x : T`, dan `modport`. Posisi (line, col) nama untuk
/// E2007 duplikat interface (F26).
#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    pub name: String,
    /// Port interface — di-emit sebagai deklarasi signal (arah dipakai
    /// hanya oleh modport yang merujuk ke nama tsb).
    pub ports: Vec<Port>,
    /// `sig a, b : T` — (nama, tipe, line, col)
    pub sigs: Vec<(Vec<String>, MvType, usize, usize)>,
    pub modports: Vec<Modport>,
    pub line: usize,
    pub col: usize,
}

/// Package `.mv`: `package counter_pkg { ... }`. Posisi (line, col) nama untuk
/// E2007 duplikat package (F11).
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    pub typedefs: Vec<Typedef>,
    /// (nama, tipe opsional, nilai)
    pub consts: Vec<(String, Option<MvType>, Expr)>,
    pub line: usize,
    pub col: usize,
}

/// Satu file `.mv` (hasil parse).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MvFile {
    /// Typedef level file (keluar di `$unit` .svh)
    pub typedefs: Vec<Typedef>,
    pub packages: Vec<Package>,
    /// `interface name { ... }` (MIVON-HDL.md §6.10) — definisi bersama,
    /// di-emit ke `.svh`.
    pub interfaces: Vec<Interface>,
    pub modules: Vec<Module>,
    /// `program name { ... }` — testbench program (MIVON-HDL.md §7.3).
    /// Body memakai struktur module (port + item) — emisi `program ... endprogram`.
    pub programs: Vec<Module>,
    /// `class name [extends base] { ... }` (MIVON-HDL.md §8) — emisi class SV.
    pub classes: Vec<MClass>,
    pub funcs: Vec<MFunc>,
    pub tasks: Vec<MTask>,
}
