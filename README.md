# mivon — RTL Simulator untuk SystemVerilog

**Versi 0.2.9** | Rust | 609 tests | MIT

A Rust-based RTL simulator for SystemVerilog. Compiles `.sv` files through a pipeline of preprocessor → lexer → parser → AST → elaborator → IR → simulation engine → VCD/FST output.

## Pipeline

```
.sv → preprocessor → lexer → parser → AST → elaborator → IR → engine → VCD/FST
```

## Quick start

```shell
mivon run -- test/counter.sv              # simulate counter
mivon run -- test/tb_counter.sv -T 200    # with max time
mivon run -- file.sv --ast                # print AST
mivon run -- file.sv --tokens             # print tokens
```

## Project file

```
counter.sv
tb_counter.sv
```

```shell
```

## CLI flags

```
- T <N>        max simulation time
--ast          print AST (no simulation)
--tokens       print tokens (no simulation)
--top <MOD>    top-level module name
--debug        enable debug mode (breakpoints)
--deep-debug   enable + snapshots for reverse debug
--step         single-cycle execution
-I <DIR>       include directory for `include
-D <MACRO>     define macro
-f <FILE>      file list (like -f in VCS)
--coverage     print coverage report
--coverage-ucis [PATH]  export UCIS XML
-f <FILE>      file list / project file (`.f` / `.mivon`)
```

## Fitur utama

- Full 4-state logic (X/Z/0/1) dengan propagation
- IEEE 1800 12-region stratified event scheduler
- `always_ff` / `always_comb` / `always_latch` / `initial` / `final`
- `fork`/`join`/`join_any`/`join_none` concurrent execution
- `interface` + `modport`, `package` + `import`, `program` block
- OOP: class, `extends`, virtual dispatch, `super.new()`, parameterized class
- UVM: `uvm_object`, `uvm_component`, `uvm_sequence`/`sequencer`/`driver`, factory, TLM, phases
- SVA: immediate `assert`/`assume`/`cover`, concurrent property parsing
- Coverage: `covergroup`/`coverpoint`/`cross`/`bins` + UCIS XML export
- Constraint randomize: `rand`/`randc`, `constraint`, `solve...before`, `dist`
- DPI-C import, `bind` construct, `clocking` block, `config`/`libmap`/`use`
- SDF annotation, FST waveform (zlib compression), VCD hierarchical dump
- `mailbox`/`semaphore`/`process` class, `randcase`/`randsequence`
- `$sformatf`, `$fopen`/`$fclose`/`$fdisplay`/`$fwrite`/`$fstrobe`/`$fmonitor`/`$fscanf`/`$fread`
- `$urandom`/`$random(seed)`/`$urandom_range`/`$realtime`
- Debugger: breakpoint, watchpoint, step, reverse debug, timeline, hierarchy tree
- Parallel simulation framework: `ParallelConfig`, `evaluate_expr_simple`, `evaluate_stmt_block_parallel`, `parallel_snapshot` (rayon-based)
- JIT stub: basic expression compilation via `JITCompiler` (Cranelift integration planned)
- UVM macros: `uvm_macros.svh` — info/warning/error/fatal, factory utils, field macros
- Picorv32 RISC-V CPU (3049 LOC) compilation + simulation completed
- AXI + Wishbone wrapper simulation via `--top`
- IEEE 1800 compliance  fitur relevan RTL

## Build & test

```shell
cargo build
cargo test
cargo test <test_name>
```

No CI, no lint, no typecheck shortcuts. Just `cargo test`. 1634 tests pass.

## CI/CD

### Automated Release Updates
The project uses a sophisticated GitHub Actions workflow (`.github/workflows/release-update.yml`) that:
- **Validates changes** before proceeding (only allows updates on main branch or explicit manual triggers)
- **Builds Mivon binaries** automatically on each push to main
- **Updates installation documentation** in the landing page
- **Detects internal updates** and verifies binary functionality
- **Creates GitHub Releases** with changelog and binaries
- **Maintains cache** and cleans up temporary files

### Manual Workflow Triggers

```bash
# Trigger manual landing page update
gh workflow run release-update.yml -f update_landing=true

# Run workflow manually
gh workflow run release-update.yml
```

## Installation

### Automatic Installation (Recommended)

```bash
# Install latest stable release automatically
curl -fsSL https://raw.githubusercontent.com/mivonsim/mivon/main/install.sh -o install.sh
sudo bash install.sh
```

### Build from Source

```bash
git clone https://github.com/mivonsim/mivon.git
cd mivon
cargo build --release
```

The binary will be at `target/release/mivon`. Add it to your PATH:

```bash
# Add to ~/.bashrc or ~/.zshrc
export PATH="$HOME/mivon/target/release:$PATH"
```

Then run:

```bash
mivon --help
```

## CLI Tools (`crates/mivon-tools/`, subcommand `mivon <tool>`)