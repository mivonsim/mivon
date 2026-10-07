# mivon-fuzz LRM Judge — Desain & Arsitektur

LRM-based conformance judge untuk mivon. Hakim tunggal adalah IEEE 1800,
bukan simulator lain.

Prinsip utama:

```
LRM → Judge → Verdict → Mivon
```

Bukan:

```
Mivon → dibandingkan Verilator/VCS/Questa → dianggap benar
```

Simulator lain boleh memberi sinyal adanya perbedaan, tetapi hanya model
aturan LRM yang mempunyai kewenangan menyatakan "Mivon salah". Tanpa
pemisahan ini mivon-fuzz hanya menjadi differential tester.

---

## 1. Arsitektur Keseluruhan

```
                     ┌─────────────────────────┐
                     │      IEEE 1800 LRM      │
                     │                         │
                     │  syntax                 │
                     │  grammar                │
                     │  semantics              │
                     │  typing                 │
                     │  elaboration            │
                     │  scheduling             │
                     │  assertions             │
                     │  constraints            │
                     │  coverage               │
                     │  legality               │
                     └────────────┬────────────┘
                                  │
                                  ▼
                     ┌─────────────────────────┐
                     │       LRM JUDGE         │
                     │                         │
                     │  Rule DB                │
                     │  Rule Resolver          │
                     │  Semantic Model         │
                     │  Constraint Evaluator   │
                     │  Expected Behavior      │
                     │  Ambiguity Resolver     │
                     └────────────┬────────────┘
                                  │
                          LRM expectation
                                  │
                                  ▼
┌──────────────────┐    ┌─────────────────────────┐
│  FUZZ GENERATOR  │───►│         MIVON           │
│                  │    │                         │
│  AST mutation    │    │  lexer                  │
│  grammar fuzz    │    │  parser                 │
│  semantic fuzz   │    │  elaborator             │
│  state fuzz      │    │  simulator              │
│  timing fuzz     │    └───────────┬─────────────┘
└──────────────────┘                │
                             actual behavior
                                    │
                                    ▼
                        ┌─────────────────────────┐
                        │    OBSERVATION ENGINE   │
                        │                         │
                        │  diagnostics            │
                        │  AST                    │
                        │  elaboration            │
                        │  values                 │
                        │  events                 │
                        │  scheduling             │
                        │  exit status            │
                        └───────────┬─────────────┘
                                    │
                                    ▼
                        ┌─────────────────────────┐
                        │     VERDICT ENGINE      │
                        │                         │
                        │  LRM PASS               │
                        │  LRM VIOLATION          │
                        │  INVALID TEST           │
                        │  LRM UNDEFINED          │
                        │  IMPLEMENTATION DEP.    │
                        │  LRM AMBIGUOUS          │
                        │  JUDGE INSUFFICIENT     │
                        └─────────────────────────┘
```

---

## 2. Struktur Crate — Sesuai Workspace Mivon

Semua crate baru masuk `crates/` root workspace, sejajar dengan crate yang
sudah ada. Pola penamaan mengikuti konvensi yang berlaku: prefix `mivon-`
untuk crate yang berhubungan langsung dengan pipeline mivon, nama deskriptif
untuk crate yang berdiri mandiri.

```
crates/
│
│   ── sudah ada ──────────────────────────────────────────────
├── mivon-core/
├── mivon-ast/
├── mivon-ir/
├── mivon-parser/
├── mivon-elaboration/
├── mivon-compiler/
├── mivon-simulator/
├── mivon-mv/
├── mivon-api/
├── mivon-tools/
├── mivon-env/
├── mivon-tests/
│   (... crate lainnya)
│
│   ── baru: LRM Judge system ──────────────────────────────────
├── mivon-fuzz/             ← tetap ada, jadi orchestrator utama
│   src/
│   ├── lib.rs              ← re-export crate-crate di bawah
│   ├── main.rs             ← CLI: run/replay/triage/minimize/verify/report
│   ├── corpus.rs           ← tetap (seed loader)
│   ├── deps.rs             ← tetap (dependency resolver)
│   ├── directed.rs         ← tetap (directed mutation)
│   ├── minimize.rs         ← tetap (ddmin)
│   ├── mutator.rs          ← tetap (19 operasi mutasi)
│   ├── oracle.rs           ← tetap (O1-O6 oracle existing)
│   ├── oracle_icarus.rs    ← tetap (differential secondary evidence)
│   ├── runner.rs           ← tetap (subprocess runner)
│   ├── triage.rs           ← tetap (dedup + render)
│   └── validate.rs         ← tetap (determinism + differential)
│
├── lrm-model/              ← tipe machine-readable LRM (rule, clause, predicate)
├── lrm-rules/              ← database aturan LRM per domain
├── lrm-judge/              ← evaluator: rule + observation + state → verdict
├── mivon-observer/         ← instrumentasi mivon → MivonObservation terstruktur
└── fuzz-verdict/           ← tipe Verdict, Confidence, Evidence, Provenance
```

Dependency satu arah — tidak boleh ada siklus:

```
mivon-api  ←── mivon-observer  (observer memanggil compile/sim mivon-api)
    │
    └───────────────────────────────────────┐
                                            │
lrm-model                                  │
    │                                       │
    ▼                                       │
lrm-rules                                  │
    │                                       │
    ▼                                       ▼
lrm-judge  ←──────────────────── mivon-observer
    │
    ▼
fuzz-verdict
    ▲
    │
mivon-fuzz  (orchestrator — dep ke semua di atas)
```

`lrm-judge` tidak boleh mempunyai dependency ke iverilog, verilator, VCS,
atau simulator lain. Simulator lain adalah secondary evidence di lapisan
terpisah (`oracle_icarus.rs` di `mivon-fuzz`) — bukan di dalam hakim.

---

## 3. Cargo.toml Workspace

Tambahan di `members` workspace root `Cargo.toml`:

```toml
[workspace]
members = [
    # ... crate yang sudah ada ...
    "crates/mivon-fuzz",
    "crates/lrm-model",
    "crates/lrm-rules",
    "crates/lrm-judge",
    "crates/mivon-observer",
    "crates/fuzz-verdict",
]
```

Setiap crate baru mempunyai `Cargo.toml` sendiri dengan pola yang sama:

```toml
# crates/lrm-model/Cargo.toml
[package]
name = "lrm-model"
version = "0.1.0"
edition = "2021"
description = "Machine-readable IEEE 1800 LRM rule model untuk mivon-fuzz"

[lib]
name = "lrm_model"
path = "src/lib.rs"
```

```toml
# crates/lrm-rules/Cargo.toml
[package]
name = "lrm-rules"
version = "0.1.0"
edition = "2021"
description = "Database aturan LRM per domain untuk mivon-fuzz"

[dependencies]
lrm-model = { path = "../lrm-model" }
```

```toml
# crates/lrm-judge/Cargo.toml
[package]
name = "lrm-judge"
version = "0.1.0"
edition = "2021"
description = "LRM conformance evaluator — hakim mivon vs IEEE 1800"

[dependencies]
lrm-model  = { path = "../lrm-model" }
lrm-rules  = { path = "../lrm-rules" }
fuzz-verdict = { path = "../fuzz-verdict" }
```

```toml
# crates/mivon-observer/Cargo.toml
[package]
name = "mivon-observer"
version = "0.1.0"
edition = "2021"
description = "Instrumentasi mivon → MivonObservation terstruktur"

[features]
dev = ["dep:mivon-api", "dep:mivon-ir"]

[dependencies]
mivon-api = { path = "../mivon-api", optional = true }
mivon-ir  = { path = "../mivon-ir", optional = true }
lrm-model = { path = "../lrm-model" }
```

```toml
# crates/fuzz-verdict/Cargo.toml
[package]
name = "fuzz-verdict"
version = "0.1.0"
edition = "2021"
description = "Tipe Verdict, Confidence, Evidence, Provenance untuk mivon-fuzz"

[dependencies]
lrm-model = { path = "../lrm-model" }
```

`mivon-fuzz` menambah dependency ke semua crate baru di bawah feature `dev`:

```toml
# crates/mivon-fuzz/Cargo.toml — tambahan
[features]
dev = [
    "dep:mivon-api",
    "dep:mivon-ir",
    "dep:mivon-parser",
    "dep:rand",
    "dep:serde",
    # baru
    "dep:lrm-model",
    "dep:lrm-rules",
    "dep:lrm-judge",
    "dep:mivon-observer",
    "dep:fuzz-verdict",
]

[dependencies]
# ... dep yang sudah ada ...
lrm-model      = { path = "../lrm-model",      optional = true }
lrm-rules      = { path = "../lrm-rules",      optional = true }
lrm-judge      = { path = "../lrm-judge",      optional = true }
mivon-observer = { path = "../mivon-observer", optional = true }
fuzz-verdict   = { path = "../fuzz-verdict",   optional = true }
```

---

## 4. lrm-model — Tipe Machine-Readable LRM

Jangan menyimpan LRM sebagai teks mentah. LRM sebagai teks = mesin yang
terdengar pintar tetapi runtuh di corner case. Buat semantic model yang
dapat dievaluasi secara programatik.

### 4.1 Struktur Direktori

```
crates/lrm-model/src/
├── lib.rs          ← pub use semua modul
├── rule.rs         ← LrmRule, RuleId, RuleCategory, RuleClassification
├── clause.rs       ← ClauseRef, Standard
├── predicate.rs    ← Predicate, Requirement, Constraint
├── behavior.rs     ← ExpectedBehavior, ObservedBehavior
├── state.rs        ← LrmState dan seluruh sub-model
└── value.rs        ← LrmValue, LogicBit (representasi nilai 4-state)
```

### 4.2 Tipe Inti

```rust
// rule.rs

/// Identitas satu aturan LRM.
pub struct LrmRule {
    pub id: RuleId,
    pub clause: ClauseRef,
    pub category: RuleCategory,

    /// Kapan aturan ini berlaku (predikat atas LrmState).
    pub applies_when: Predicate,

    /// Yang harus dipenuhi oleh implementasi.
    pub requires: Vec<Requirement>,

    /// Yang dilarang terjadi.
    pub forbids: Vec<Constraint>,

    /// Perilaku yang diharapkan jika aturan berlaku.
    pub expected: ExpectedBehavior,

    /// Apakah ini required, undefined, implementation-dependent, dll.
    pub classification: RuleClassification,
}

/// ID aturan — contoh: "SV-TYPE-INT-001", "SV-ELAB-PARAM-003"
pub struct RuleId(pub &'static str);

pub enum RuleCategory {
    Syntax,
    Grammar,
    Legality,
    Semantics,
    Typing,
    Elaboration,
    Scheduling,
    Assertion,
    Constraint,
    Coverage,
    SystemFunction,
    Portability,
}

/// Klasifikasi perilaku menurut LRM.
///
/// Ini krusial: tidak semua perbedaan Mivon vs expected = bug.
/// LRM sendiri yang menentukan apakah perilaku wajib, tidak terdefinisi,
/// atau bergantung implementasi.
pub enum RuleClassification {
    /// LRM mewajibkan perilaku spesifik. Penyimpangan = violation.
    RequiredBehavior,
    /// LRM menyatakan perilaku tidak terdefinisi. Mivon boleh berbeda.
    UndefinedByLrm,
    /// LRM memberi kebebasan ke implementasi. Boleh berbeda.
    ImplementationDefined,
    /// LRM ambigu. Perlu interpretasi manusia.
    LrmAmbiguous,
    /// Perilaku bergantung pada tool flags / elaboration context.
    ConditionalBehavior,
}
```

```rust
// clause.rs

/// Referensi ke klausul standar — bukan teks LRM verbatim.
pub struct ClauseRef {
    pub standard: Standard,
    /// Contoh: "6.24.1", "11.4.14", "10.6.2"
    pub section: String,
    /// Revisi standar: "2012", "2017"
    pub revision: &'static str,
}

pub enum Standard {
    Ieee1800_2012,
    Ieee1800_2017,
    Ieee1364_2005,
}
```

```rust
// predicate.rs

/// Predikat atas state — kapan aturan berlaku.
pub type Predicate = Box<dyn Fn(&LrmState) -> bool + Send + Sync>;

/// Requirement — apa yang harus dipenuhi implementasi.
pub enum Requirement {
    /// Ekspresi harus menghasilkan nilai tertentu.
    ValueEquals { signal: String, expected: LrmValue },
    /// Diagnostik wajib diterbitkan dengan lokasi file:line:col.
    DiagnosticRequired { code: String, severity: Severity },
    /// Diagnostik tidak boleh diterbitkan.
    DiagnosticForbidden { code: String },
    /// Elaborasi harus berhasil.
    ElaborationSucceeds,
    /// Elaborasi harus gagal dengan kode tertentu.
    ElaborationFails { code: String },
    /// Urutan event harus sesuai model scheduling LRM.
    SchedulingOrder { model: SchedulingModel },
    /// Properti semantik harus terpenuhi.
    SemanticProperty(SemanticProp),
}

pub enum Severity { Error, Warning, Note }
```

```rust
// value.rs

/// Nilai 4-state LRM — representasi canonical.
pub enum LrmValue {
    /// Integer 4-state: bit per bit 0/1/X/Z, lebar eksplisit.
    FourState { bits: Vec<LogicBit>, width: u32 },
    Real(f64),
    Str(String),
    /// Nilai tidak terdefinisi oleh LRM untuk kasus ini.
    LrmUndefined,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogicBit { Zero, One, X, Z }
```

### 4.3 LrmState — State yang Dievaluasi Hakim

Hakim mengevaluasi aturan terhadap state ini. Bukan terhadap stdout mentah.

```rust
// state.rs

pub struct LrmState {
    pub scopes: ScopeGraph,
    pub symbols: SymbolTable,
    pub types: TypeSystem,
    pub elaborated_design: DesignModel,
    pub processes: ProcessModel,
    pub scheduler: SchedulerModel,
}

pub struct ScopeGraph {
    pub root: ScopeId,
    pub nodes: HashMap<ScopeId, ScopeNode>,
    pub parent: HashMap<ScopeId, ScopeId>,
}

pub struct SymbolTable {
    pub entries: HashMap<SymbolId, SymbolEntry>,
    pub by_name: HashMap<(ScopeId, String), SymbolId>,
}

pub struct TypeSystem {
    pub types: HashMap<TypeId, SvType>,
    pub bindings: HashMap<SymbolId, TypeId>,
    pub coercions: Vec<CoercionRecord>,
}

pub struct DesignModel {
    pub hierarchy: HierarchyTree,
    pub parameters: HashMap<String, ParameterBinding>,
    pub generates: Vec<GenerateBlock>,
    pub port_bindings: Vec<PortBinding>,
}

pub struct ProcessModel {
    pub always_blocks: Vec<AlwaysProcess>,
    pub initial_blocks: Vec<InitialProcess>,
    pub final_blocks: Vec<FinalProcess>,
}

pub struct SchedulerModel {
    /// Active / NBA / Observed / Reactive / Postponed (LRM §4.4)
    pub regions: Vec<SchedulerRegion>,
    pub time_steps: Vec<TimeStep>,
}
```

---

## 5. lrm-rules — Database Aturan Per Domain

```
crates/lrm-rules/src/
├── lib.rs              ← RuleRegistry + pub use semua modul domain
├── registry.rs         ← RuleRegistry: register, by_id, applicable
│
├── syntax/
│   ├── mod.rs
│   ├── grammar.rs      ← aturan grammar BNF SV
│   └── legality.rs     ← legality rules (apa yang syntactically legal)
│
├── semantics/
│   ├── mod.rs
│   ├── expression.rs   ← evaluasi ekspresi, precedence, 4-state
│   ├── declaration.rs  ← aturan deklarasi variabel/net/port
│   └── statement.rs    ← aturan statement prosedural
│
├── types/
│   ├── mod.rs
│   ├── integral.rs     ← int, integer, logic, bit, reg, wire
│   ├── packed.rs       ← packed array, packed struct, packed union
│   ├── unpacked.rs     ← unpacked array, dynamic, queue, assoc
│   ├── struct.rs       ← struct semantics + assignment pattern
│   └── enum.rs         ← enum member, cast, comparison
│
├── elaboration/
│   ├── mod.rs
│   ├── hierarchy.rs    ← instance tree, port connection
│   ├── generate.rs     ← generate for/if/case expansion
│   ├── parameter.rs    ← param propagation, type param, localparam
│   └── binding.rs      ← port binding, implicit connect
│
├── scheduling/
│   ├── mod.rs
│   ├── region.rs       ← Active/NBA/Observed/Reactive/Postponed
│   ├── event.rs        ← event trigger, @(posedge), wait
│   └── race.rs         ← race condition
│
├── assertions/
│   ├── mod.rs
│   ├── immediate.rs    ← assert, assume, cover (immediate)
│   └── concurrent.rs  ← property, sequence, SVA
│
└── system/
    ├── mod.rs
    ├── tasks.rs        ← $display, $monitor, $fopen, $finish
    ├── functions.rs    ← $clog2, $bits, $size, $left, $right
    ├── math.rs         ← $sqrt, $ln, $exp, $pow, trigonometri
    └── conversion.rs   ← $rtoi, $itor, $realtobits, $bitstoreal
```

```rust
// registry.rs

pub struct RuleRegistry {
    rules: HashMap<RuleId, LrmRule>,
    by_category: HashMap<RuleCategory, Vec<RuleId>>,
    by_clause: HashMap<String, Vec<RuleId>>,
}

impl RuleRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, rule: LrmRule);
    pub fn by_id(&self, id: &str) -> Option<&LrmRule>;
    pub fn for_category(&self, cat: RuleCategory) -> &[RuleId];
    pub fn for_clause(&self, section: &str) -> &[RuleId];
    /// Kembalikan semua aturan yang applies_when(&state) == true.
    pub fn applicable<'a>(&'a self, state: &LrmState) -> Vec<&'a LrmRule>;
}
```

Setiap modul domain mendaftarkan aturannya ke registry lewat fungsi `register_*`:

```rust
// elaboration/parameter.rs

pub fn register(registry: &mut RuleRegistry) {
    registry.register(LrmRule {
        id: RuleId("SV-ELAB-PARAM-003"),
        clause: ClauseRef {
            standard: Standard::Ieee1800_2017,
            section: "23.10.1".into(),
            revision: "2017",
        },
        category: RuleCategory::Elaboration,
        applies_when: Box::new(|state| {
            // Berlaku bila ada instance dengan parameter override
            state.elaborated_design.parameters
                .values()
                .any(|b| b.is_overridden)
        }),
        requires: vec![
            Requirement::SemanticProperty(SemanticProp::GenerateIterationsMatchParam),
        ],
        forbids: vec![],
        expected: ExpectedBehavior::GenerateExpandsWithOverriddenParam,
        classification: RuleClassification::RequiredBehavior,
    });
}
```

---

## 6. lrm-judge — Evaluator Aturan

```
crates/lrm-judge/src/
├── lib.rs              ← LrmJudge, JudgeReport, pub use
├── judge.rs            ← LrmJudge::judge() — aggregator utama
│
├── syntax/
│   ├── mod.rs
│   ├── grammar.rs
│   └── legality.rs
│
├── semantics/
│   ├── mod.rs
│   ├── expression.rs
│   ├── declaration.rs
│   └── statement.rs
│
├── types/
│   ├── mod.rs
│   ├── integral.rs
│   ├── packed.rs
│   ├── unpacked.rs
│   ├── struct.rs
│   └── enum.rs
│
├── elaboration/
│   ├── mod.rs
│   ├── hierarchy.rs
│   ├── generate.rs
│   ├── parameter.rs
│   └── binding.rs
│
├── scheduling/
│   ├── mod.rs
│   ├── region.rs
│   ├── event.rs
│   └── race.rs
│
├── assertions/
│   ├── mod.rs
│   ├── immediate.rs
│   └── concurrent.rs
│
└── system/
    ├── mod.rs
    ├── tasks.rs
    ├── functions.rs
    ├── math.rs
    └── conversion.rs
```

### 6.1 Trait Evaluator

```rust
// lib.rs

pub trait LrmRuleEvaluator: Send + Sync {
    fn evaluate(
        &self,
        rule: &LrmRule,
        testcase: &TestCase,
        observation: &MivonObservation,
        state: &LrmState,
    ) -> RuleResult;
}

pub struct RuleResult {
    pub rule: RuleId,
    pub verdict: RuleVerdict,
    pub evidence: Vec<Evidence>,
    /// Penjelasan langkah per langkah — dapat ditelusuri ke klausul LRM.
    pub explanation: String,
}

pub enum RuleVerdict {
    Satisfied,
    Violated { reason: String },
    NotApplicable,
    Inconclusive { reason: String },
}
```

### 6.2 Judge Aggregator

```rust
// judge.rs

pub struct LrmJudge {
    registry: RuleRegistry,
    evaluators: HashMap<RuleCategory, Box<dyn LrmRuleEvaluator>>,
}

impl LrmJudge {
    pub fn new(registry: RuleRegistry) -> Self;

    /// Evaluasi semua aturan yang berlaku untuk testcase + observasi ini.
    pub fn judge(
        &self,
        testcase: &TestCase,
        observation: &MivonObservation,
        state: &LrmState,
    ) -> JudgeReport;
}

pub struct JudgeReport {
    pub testcase_id: String,
    pub verdict: Verdict,
    pub confidence: JudgeConfidence,
    pub rule_results: Vec<RuleResult>,
    pub evidence: Vec<Evidence>,
    pub provenance: Provenance,
}
```

### 6.3 Prinsip Evaluasi

Tidak boleh ada verdict tanpa provenance:

```rust
// SALAH — tidak ada ketertelusuran
if weird_behavior {
    fail();
}

// BENAR — setiap verdict mengacu aturan LRM secara eksplisit
if rule.applies_when(&state)
    && rule.requires.iter().any(|r| !r.satisfied_by(&observation))
{
    RuleResult {
        rule: rule.id.clone(),
        verdict: RuleVerdict::Violated {
            reason: format!(
                "Rule {} (§{}) dilanggar: {}",
                rule.id.0,
                rule.clause.section,
                build_explanation(rule, observation, state)
            ),
        },
        evidence: collect_evidence(rule, observation, state),
        explanation: build_explanation(rule, observation, state),
    }
}
```

---

## 7. mivon-observer — Instrumentasi Mivon

Observer mengubah output mivon yang tidak terstruktur menjadi
`MivonObservation` yang dapat dievaluasi hakim.

```
crates/mivon-observer/src/
├── lib.rs          ← MivonObservation, pub use
├── runner.rs       ← jalankan mivon (subprocess atau in-process via mivon-api)
├── collector.rs    ← kumpulkan stdout/stderr/exit → MivonObservation
├── elab.rs         ← ekstrak ElaborationSnapshot dari mivon-api
├── trace.rs        ← ekstrak RuntimeTrace + ValueObservation
└── state.rs        ← bangun LrmState dari IrDesign (untuk judge)
```

```rust
// lib.rs

pub struct MivonObservation {
    pub exit_status: ExitStatus,
    pub diagnostics: Vec<ObsDiagnostic>,
    pub elaboration: Option<ElaborationSnapshot>,
    pub runtime: Option<RuntimeTrace>,
    pub values: Vec<ValueObservation>,
    pub scheduling: Vec<SchedulingObservation>,
}

pub struct ObsDiagnostic {
    pub severity: ObsSeverity,
    /// Kode diagnostik mivon — contoh: "E1002", "WR0102"
    pub code: String,
    pub message: String,
    pub location: Option<ObsLocation>,
}

pub struct ObsLocation {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

pub struct ElaborationSnapshot {
    pub hierarchy: Vec<HierarchyNode>,
    pub parameters: Vec<ParameterBinding>,
    pub signals: Vec<SignalDecl>,
    pub generate_expansions: Vec<GenerateExpansion>,
    pub port_bindings: Vec<PortBinding>,
}

pub struct RuntimeTrace {
    pub time_steps: Vec<TimeStepRecord>,
    pub final_values: HashMap<String, LrmValue>,
}

pub struct ValueObservation {
    pub signal: String,
    pub time: u64,
    pub value: LrmValue,
}

pub enum ExitStatus {
    Ok,
    CleanError,
    Panic,
    Hang,
    Crash { code: i32 },
}
```

Pipeline observasi yang dicatat:

```
parse
  │
  ▼
AST
  │
  ▼
semantic / type resolution
  │
  ▼
elaboration  ← ElaborationSnapshot
  │
  ▼
process creation
  │
  ▼
event scheduling  ← SchedulingObservation
  │
  ▼
assignment (active / NBA)  ← ValueObservation per time step
  │
  ▼
final state  ← RuntimeTrace.final_values
```

`state.rs` membangun `LrmState` dari `IrDesign` mivon — jembatan antara
representasi internal mivon dan model LRM yang dipakai hakim:

```rust
// state.rs

/// Bangun LrmState dari IrDesign hasil elaborasi mivon.
/// Dipakai lrm-judge untuk mengevaluasi aturan terhadap design yang konkret.
pub fn build_lrm_state(design: &mivon_ir::IrDesign) -> LrmState {
    LrmState {
        scopes: build_scope_graph(design),
        symbols: build_symbol_table(design),
        types: build_type_system(design),
        elaborated_design: build_design_model(design),
        processes: build_process_model(design),
        scheduler: SchedulerModel::default(),
    }
}
```

---

## 8. fuzz-verdict — Sistem Keputusan

```
crates/fuzz-verdict/src/
├── lib.rs          ← pub use semua
├── verdict.rs      ← Verdict enum
├── confidence.rs   ← JudgeConfidence enum
├── evidence.rs     ← Evidence, ApplicabilityProof
├── provenance.rs   ← Provenance, ketertelusuran penuh
└── testcase.rs     ← TestCase, TestManifest, MutationRecord
```

### 8.1 Verdict

```rust
// verdict.rs

pub enum Verdict {
    /// Mivon sesuai LRM untuk testcase ini.
    Pass,

    /// Mivon melanggar aturan LRM yang wajib.
    Violation {
        rule: RuleId,
        clause: ClauseRef,
        expected: ExpectedBehavior,
        observed: ObservedBehavior,
    },

    /// Testcase tidak valid — tidak bisa menguji aturan yang dimaksud.
    InvalidTest { reason: String },

    /// LRM tidak mendefinisikan perilaku untuk kasus ini.
    /// Mivon boleh berperilaku apa saja — bukan bug.
    UndefinedByLrm {
        rule: RuleId,
        clause: ClauseRef,
    },

    /// LRM memberi kebebasan ke implementasi.
    /// Mivon tidak salah meski berbeda dari simulator lain.
    ImplementationDependent {
        rule: RuleId,
        clause: ClauseRef,
    },

    /// Klausul LRM ambigu. Membutuhkan interpretasi manusia.
    LrmAmbiguous {
        clause: ClauseRef,
        interpretations: Vec<String>,
    },

    /// Judge tidak cukup informasi untuk memberi keputusan.
    JudgeInsufficient {
        reason: String,
        missing: Vec<String>,
    },

    /// Mivon crash/panic/hang — bukan masalah conformance tapi tetap bug.
    MivonInternalFailure {
        kind: FailureKind,
        detail: String,
    },
}

pub enum FailureKind { Panic, Hang, Crash, Abort }
```

### 8.2 Confidence

```rust
// confidence.rs

pub enum JudgeConfidence {
    /// Terbukti secara formal dari aturan LRM + observasi lengkap.
    Proven,
    /// Bukti kuat, tidak ada ambiguitas signifikan.
    Strong,
    /// Bergantung pada asumsi yang masuk akal.
    Conditional { assumptions: Vec<String> },
    /// Informasi tidak cukup untuk keputusan kuat.
    Insufficient,
}
```

Aturan: `Violation` + `Insufficient` **tidak disimpan sebagai bug**. Disimpan
sebagai finding untuk review manual.

### 8.3 Provenance

```rust
// provenance.rs

/// Ketertelusuran penuh — setiap verdict dapat ditelusuri ke klausul LRM.
pub struct Provenance {
    pub rule: RuleId,
    pub clause: ClauseRef,
    /// Bukti bahwa aturan berlaku untuk testcase ini.
    pub applicability: ApplicabilityProof,
    pub expected: ExpectedBehavior,
    pub observed: ObservedBehavior,
}

pub struct ApplicabilityProof {
    /// Setiap kondisi yang membuat aturan berlaku, beserta nilai saat evaluasi.
    pub conditions: Vec<(String, bool)>,
    /// true jika semua kondisi terpenuhi.
    pub applies: bool,
}
```

### 8.4 TestCase

```rust
// testcase.rs

pub struct TestCase {
    pub id: String,
    pub source: String,
    pub manifest: TestManifest,
    pub provenance: TestProvenance,
}

pub struct TestManifest {
    pub test_id: String,
    pub generated_by: GeneratorKind,
    pub target_rules: Vec<String>,
    pub features: Vec<String>,
    pub lrm_clauses: Vec<String>,
}

pub enum GeneratorKind {
    /// Generator mengetahui aturan yang diserang.
    RuleDirected,
    /// Generator acak — hakim menentukan aturan setelah observasi.
    BlindStructural,
    /// Transformasi semantically equivalent (metamorphic).
    Metamorphic { transformation: String },
}

pub struct TestProvenance {
    pub seed: u64,
    pub mutation_chain: Vec<MutationRecord>,
    pub parent_test: Option<String>,
    pub rule_targeted: Option<String>,
}

pub struct MutationRecord {
    pub op: String,
    pub detail: String,
}
```

---

## 9. Dua Jenis Generator

### 9.1 Rule-Directed Fuzzing

```
LRM rule
    │
    ▼
feature extraction
    │
    ▼
test template
    │
    ▼
mutation (mutator.rs di mivon-fuzz)
    │
    ▼
TestCase dengan manifest lengkap
    │
    ▼
Mivon
```

Generator mengetahui aturan yang diserang. Semua mutasi relevan terhadap
domain aturan tersebut. Contoh untuk aturan konversi tipe → serangan pada:
`signed`, `unsigned`, `packed`, `unpacked`, `width`, `cast`, `context`,
`constant expression`, `parameter`.

### 9.2 Blind Structural Fuzzing

```
grammar / corpus seed
    │
    ▼
mutation 0-4x (mutator.rs) + directed mutation 30% (directed.rs)
    │
    ▼
TestCase (target_rules = [] — belum diketahui)
    │
    ▼
Mivon
    │
    ▼
lrm-judge menentukan aturan mana yang terkena
```

Generator tidak tahu aturan yang terkena. `lrm-judge` menentukan aturan
setelah observasi. Lebih kasar, tapi menemukan area yang tidak terpikirkan
oleh rule-directed.

---

## 10. Metamorphic LRM Testing

Untuk kasus di mana expected output sulit ditentukan secara eksplisit.

```
         Test A
           │
           ▼
     LRM transformation
     (dijamin semantically equivalent oleh LRM)
           │
           ▼
         Test B
           │
     ┌─────┴─────┐
     ▼           ▼
   Mivon A     Mivon B
     │           │
     └─────┬─────┘
           ▼
       LRM Judge
           │
           ▼
        Verdict
```

Jika LRM menjamin dua konstruksi semantically equivalent:

```
Mivon(A) ≠ Mivon(T(A))  →  kandidat bug
```

Contoh transformasi yang LRM jamin ekuivalen:

| Transformasi | LRM Basis |
|---|---|
| `a + b` ↔ `b + a` (integer, tanpa overflow) | §11.4.3 |
| `always_ff @(posedge clk) q <= d` ↔ NBA assignment semantic | §10.4.2 |
| `if (x) A else B` ↔ `case (x) 1: A default: B endcase` | §12.5 |
| Named port connect ↔ ordered port connect (posisi cocok) | §23.3.2 |
| Explicit cast `int'(x)` ↔ implicit conversion yang setara | §6.24 |

---

## 11. Differential Sebagai Secondary Evidence

Simulator lain boleh digunakan. Posisinya:

```
              LRM
               │
               ▼
          ┌─────────┐
          │  JUDGE  │
          └────┬────┘
               │ authority
               ▼
             Mivon
               │
        ┌──────┴──────┐
        │             │
        ▼             ▼
    Verilator       VCS
   (secondary)   (secondary)
```

Contoh keputusan:

```
Mivon     = X
Verilator = X
VCS       = Y
LRM       = X adalah required behavior (§6.24.1)

→ Mivon PASS, VCS adalah yang diverge
```

```
Mivon     = X
Verilator = Y
VCS       = Y
LRM       = Y adalah required behavior (§6.24.1)

→ Mivon VIOLATION (bukan karena kalah 1 vs 2, tapi LRM mengatakan Y)
```

Differential di `oracle_icarus.rs` (`mivon-fuzz`) tetap ada sebagai sinyal
"ada yang perlu dilihat lebih jauh" — bukan sebagai hakim.

---

## 12. Tingkatan Hakim L0–L10

Dibangun bertahap. Target awal: L0–L4.

| Level | Nama | Yang Dinilai | Crate Domain |
|---|---|---|---|
| L0 | Syntax Judge | grammar, legal syntax, BNF | `lrm-rules/syntax/` |
| L1 | Semantic Judge | scope, binding, makna konstruksi | `lrm-rules/semantics/` |
| L2 | Type Judge | typing, conversion, sign, width | `lrm-rules/types/` |
| L3 | Elaboration Judge | hierarchy, generate, parameter | `lrm-rules/elaboration/` |
| L4 | Execution Judge | blocking/non-blocking, prosedural | `lrm-rules/semantics/statement.rs` |
| L5 | Scheduling Judge | event regions, delta cycles, NBA | `lrm-rules/scheduling/` |
| L6 | 4-state Judge | propagasi 0/1/X/Z, operasi 4-state | `lrm-rules/semantics/expression.rs` |
| L7 | Assertion Judge | SVA, immediate vs concurrent | `lrm-rules/assertions/` |
| L8 | Constraint Judge | randomization, constraint solving | (future) |
| L9 | Coverage Judge | covergroup, coverpoint, cross | (future) |
| L10 | Cross-domain Judge | interaksi seluruh domain semantik | (future) |

Prioritas: L0 → L1 → L2 → L3 → L4 → L5 → L6 → L7 → L8 → L9 → L10.
L0–L4 sudah cukup untuk menemukan mayoritas bug mivon saat ini.

---

## 13. Format Output Bug

```
╔══════════════════════════════════════════════╗
║           MIVON LRM VIOLATION                ║
╚══════════════════════════════════════════════╝

Test:          FZ-00001842
Phase:         ELABORATION
Rule:          SV-ELAB-PARAM-003
LRM:           IEEE 1800-2017 §23.10.1
Classification: REQUIRED BEHAVIOR

Expected:
    Parameter override W=16 harus memengaruhi generate for-loop
    (iterasi = 16, bukan nilai default 4)

Observed:
    Mivon menghasilkan 4 iterasi terlepas dari override W=16

Verdict:       VIOLATION
Confidence:    PROVEN

Evidence:
    elaboration.parameters["u_sub.W"] = 16  (override tercatat)
    elaboration.generates["u_sub.gen_loop"].iterations = 4  (salah)
    expected iterations = 16

Provenance:
    Rule:       SV-ELAB-PARAM-003
    Clause:     IEEE 1800-2017 §23.10.1
    Applicable: YES
      - ada instance dengan parameter override → true
      - ada generate for-loop di module tersebut → true
    Expected:   iterations = override_value (16)
    Observed:   iterations = default_value (4)

Mivon artifact:   .mivon-fuzz-bugs/bug_0042_violation.sv
Minimal testcase: .mivon-fuzz-bugs/bug_0042_violation.min.sv
```

---

## 14. Integrasi dengan mivon-fuzz yang Sudah Ada

`mivon-fuzz/src/lib.rs` menambah target `Target::Lrm` yang mengalirkan
testcase ke pipeline judge baru tanpa mengubah oracle O1–O6 yang sudah ada:

```
mivon-fuzz (orchestrator)
    │
    ├── corpus.rs        ← seed loader (tidak berubah)
    ├── mutator.rs       ← 19 operasi (tidak berubah)
    ├── directed.rs      ← directed mutation (tidak berubah)
    ├── oracle.rs        ← O1-O6 (tidak berubah)
    ├── oracle_icarus.rs ← differential secondary (tidak berubah)
    │
    └── target::Lrm      ← BARU: alirkan ke judge pipeline
           │
           ▼
       mivon-observer    ← jalankan mivon, kumpulkan MivonObservation
           │
           ▼
       lrm-judge         ← evaluasi aturan LRM
           │
           ▼
       fuzz-verdict      ← Verdict + Confidence + Provenance
```

`oracle.rs` mendapat fungsi baru `evaluate_lrm()` yang memanggil
`mivon_observer::run()` → `lrm_judge::judge()` → `fuzz_verdict::Verdict`.

---

## 15. Aturan Desain yang Tidak Boleh Dilanggar

1. **`lrm-judge` tidak boleh import simulator lain.** Tidak ada dependency
   ke iverilog, verilator, VCS di dalam crate `lrm-judge`, `lrm-rules`,
   `lrm-model`, atau `fuzz-verdict`.

2. **Setiap verdict harus punya provenance.** Tidak ada verdict tanpa
   referensi `RuleId` + `ClauseRef` yang eksplisit.

3. **`RuleClassification` menentukan apakah perbedaan adalah bug.**
   `UndefinedByLrm` dan `ImplementationDefined` → perbedaan Mivon vs
   expected bukan violation, tidak disimpan sebagai bug.

4. **`Violation` + `Insufficient` tidak disimpan sebagai bug.**
   Disimpan sebagai finding untuk review manual.

5. **Setiap testcase mempunyai manifest.** `target_rules` boleh kosong
   untuk blind structural fuzzing — tapi field selalu ada.

6. **L0–L4 lebih dulu.** Jangan membangun L8 sebelum L3 solid.

7. **Metamorphic transformation harus punya `ClauseRef`.** Tidak ada
   transformasi "feel-like-equivalent" tanpa referensi klausul LRM.

8. **`oracle_icarus.rs` tetap secondary evidence, bukan authority.**
   Mivon berbeda dari Verilator tidak otomatis berarti Mivon salah.
   LRM yang memutuskan.

9. **Semua crate baru ikut konvensi workspace mivon.** Masuk `crates/`
   root, didaftarkan di `members` workspace `Cargo.toml`, pola penamaan
   konsisten, dependency satu arah.
