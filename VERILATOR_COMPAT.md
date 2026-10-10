# Verilator Compatibility Guide — Mivon RTL Simulator

**Tanggal:** 19 Juli 2026 (diperbarui)
**Versi:** 0.3
**Standar:** Verilator 5.x + Mivon 0.2.9

---

## Ringkasan

Mivon mendukung **~70% dari Verilator-compatible subset** untuk simulasi behavioral.
Panduan ini membantu transisi kode antara Mivon dan Verilator.

**Karakteristik:**
- Mivon: interpreted AST, 4-state (X/Z), behavioral simulation
- Verilator: compiled C++, 2-state, cycle-accurate, linting + synthesis

---

## 1. Fitur yang Kompatibel (Mivon + Verilator)

### 1.1 Module & Port

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `module` | ✅ | ✅ | ANSI port list |
| `input`/`output` | ✅ | ✅ | |
| `inout` | ✅ | ✅ | Bidirectional |
| `wire`/`reg`/`logic` | ✅ | ✅ | Verilator: `logic` = `wire`/`reg` |
| `parameter`/`localparam` | ✅ | ✅ | |
| `#(parameter)` | ✅ | ✅ | Module parameterization |

### 1.2 Data Types

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `logic` | ✅ | ✅ | |
| `reg` | ✅ | ✅ | |
| `wire` | ✅ | ✅ | |
| `bit` | ✅ | ✅ | 2-state |
| `int`/`integer` | ✅ | ✅ | 32-bit |
| `byte` | ✅ | ✅ | 8-bit |
| `shortint` | ✅ | ✅ | 16-bit |
| `longint` | ✅ | ✅ | 64-bit |
| `enum` | ✅ | ✅ | Packed/unpacked |
| `struct` | ✅ | ✅ | Packed |
| `union` | ✅ | ✅ | Packed |
| `typedef` | ✅ | ✅ | |
| Packed arrays `[N:0]` | ✅ | ✅ | |
| Unpacked arrays `[0:N]` | ✅ | ✅ | |

### 1.3 Operators

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| Arithmetic `+`,`-`,`*`,`/`,`%` | ✅ | ✅ | |
| Power `**` | ✅ | ✅ | |
| Logical `&&`,`||`,`!` | ✅ | ✅ | |
| Relational `<`,`<=`,`>`,`>=` | ✅ | ✅ | |
| Equality `==`,`!=` | ✅ | ✅ | |
| Case equality `===`,`!==` | ✅ | ✅ | |
| Bitwise `&`,`|`,`^`,`~` | ✅ | ✅ | |
| Reduction `&`,`|`,`^` | ✅ | ✅ | |
| Shift `<<`,`>>`,`<<<`,`>>>` | ✅ | ✅ | |
| Concatenation `{,}` | ✅ | ✅ | |
| Replication `{n{}}` | ✅ | ✅ | |
| Conditional `? :` | ✅ | ✅ | |

### 1.4 Process Blocks

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `always_ff @(posedge clk)` | ✅ | ✅ | Sequential logic |
| `always_comb` | ✅ | ✅ | Combinational logic |
| `always_latch` | ✅ | ✅ | Latch inference |
| `always` | ✅ | ⚠️ | Verilator: prefer `always_ff`/`always_comb` |
| `assign` (continuous) | ✅ | ✅ | |
| Blocking `=` | ✅ | ✅ | In `always_comb` only |
| Non-blocking `<=` | ✅ | ✅ | In `always_ff` only |

### 1.5 Generate

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `generate if` | ✅ | ✅ | |
| `generate for` | ✅ | ✅ | |
| `generate case` | ✅ | ✅ | |
| `genvar` | ✅ | ✅ | |
| `generate begin...end` | ✅ | ✅ | Named blocks |

### 1.6 Function & Task

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `function` | ✅ | ✅ | Synthesizable subset |
| `task` | ✅ | ⚠️ | Verilator: limited (no delay) |
| `return` | ✅ | ✅ | |
| Automatic/static | ✅ | ✅ | |
| Void function | ✅ | ✅ | |
| Function ports | ✅ | ✅ | |

### 1.7 System Functions

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `$clog2` | ✅ | ✅ | |
| `$bits` | ✅ | ✅ | |
| `$size` | ✅ | ✅ | |
| `$signed`/`$unsigned` | ✅ | ✅ | |
| `$countones` | ✅ | ✅ | |
| `$onehot` | ✅ | ✅ | |
| `$onehot0` | ✅ | ✅ | |
| `$isunknown` | ✅ | ✅ | |
| `$typename` | ✅ | ✅ | |
| `$countbits` | ✅ | ✅ | |
| `$dimensions` | ✅ | ✅ | |
| `$clog2` (elaboration) | ✅ | ✅ | Compile-time |

### 1.8 Assertions (Immediate)

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `assert (expr)` | ✅ | ✅ | Immediate assert |
| `assume (expr)` | ✅ | ✅ | Immediate assume |
| `cover (expr)` | ✅ | ✅ | Immediate cover |

### 1.9 DPI-C

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `import "DPI-C" function` | ✅ | ✅ | |
| `import "DPI-C" task` | ✅ | ✅ | |
| `export "DPI-C"` | ❌ | ✅ | |

### 1.10 Package

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `package`/`endpackage` | ✅ | ✅ | |
| `import pkg::*` | ✅ | ✅ | |
| `import pkg::item` | ✅ | ✅ | |
| Package parameter | ✅ | ✅ | |
| Package typedef | ✅ | ✅ | |

### 1.11 Interface

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `interface`/`endinterface` | ✅ | ✅ | |
| `modport` | ✅ | ✅ | |
| Interface instantiation | ✅ | ✅ | |

### 1.12 Other

| Fitur | Mivon | Verilator | Catatan |
|-------|-------|-----------|---------|
| `constraint` | ✅ | ⚠️ | Verilator: random stabilization only |
| `rand`/`randc` | ✅ | ⚠️ | Verilator: rand only, no constraint solver |
| `$urandom`/`$random` | ✅ | ✅ | |
| `$urandom_range` | ✅ | ✅ | |
| `covergroup` | ✅ | ⚠️ | Verilator: functional coverage only |
| `bind` | ✅ | ⚠️ | Verilator: limited support |
| `clocking` | ✅ | ❌ | Not synthesizable |
| `config` | ✅ | ❌ | Not synthesizable |

---

## 2. Fitur yang TIDAK Kompatibel

### 2.1 Mivon punya, Verilator tidak

| Fitur | Mivon | Verilator | Alternatif |
|-------|-------|-----------|------------|
| `#delay` | ✅ | ❌ | Remove delays for synthesis |
| `@(event)` | ✅ | ⚠️ | `always_ff @(posedge clk)` |
| `posedge`/`negedge` | ✅ | ✅ | Only in `always_ff` |
| `wait(cond)` | ✅ | ❌ | Use combinational logic |
| `fork`/`join` | ✅ | ❌ | Not synthesizable |
| `initial` | ✅ | ⚠️ | For testbench only |
| `final` | ✅ | ❌ | Not synthesizable |
| `$display`/`$write` | ✅ | ❌ | Use `Verilator` lint instead |
| `$finish`/`$stop` | ✅ | ❌ | Simulation only |
| `$monitor`/`$strobe` | ✅ | ❌ | Simulation only |
| `$fopen`/`$fclose` | ✅ | ❌ | Simulation only |
| `force`/`release` | ✅ | ❌ | Not synthesizable |
| `disable` | ✅ | ❌ | Not synthesizable |
| Classes | ✅ | ❌ | Not synthesizable |
| `mailbox`/`semaphore` | ✅ | ❌ | Not synthesizable |
| UVM | ✅ | ❌ | Use Verilator testbench |
| `time`/`realtime` | ✅ | ❌ | Not synthesizable |
| `real` | ✅ | ⚠️ | Limited support |

### 2.2 Verilator punya, Mivon tidak

| Fitur | Verilator | Mivon | Catatan |
|-------|-----------|-------|---------|
| `// verilator ...` | ✅ | ❌ | Verilator directives |
| `$countones` | ✅ | ✅ | Population count |
| `$onehot` | ✅ | ✅ | One-hot detection |
| `$onehot0` | ✅ | ✅ | One-hot or zero detection |
| `$isunknown` | ✅ | ✅ | X/Z detection (4-state) |
| `$typename` | ✅ | ✅ | Type name as string |
| `$countbits` | ✅ | ✅ | Count non-zero bits |
| `$dimensions` | ✅ | ✅ | Array dimensions count |
| `$clog2` (runtime) | ✅ | ✅ | Compile-time fold + runtime eval |
| Export DPI-C | ✅ | ❌ | C→SV calling |
| SystemC export | ✅ | ❌ | |
| `/* synthesis ... */` | ✅ | ❌ | Synthesis attributes |
| `(* keep *)` | ❌ | ❌ | |
| Multidriven nets | ⚠️ | ✅ | Verilator: error |
| Tri-state logic | ❌ | ✅ | Verilator: 2-state only |
| 4-state X/Z | ❌ | ✅ | Verilator: 2-state |

---

## 3. Pola Umum yang Kompatibel

### 3.1 Sequential Logic (always_ff)

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator
module counter #(parameter WIDTH = 4)(
    input clk,
    input rst_n,
    output reg [WIDTH-1:0] count
);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n)
            count <= 0;
        else
            count <= count + 1;
    end
endmodule
```

### 3.2 Combinational Logic (always_comb)

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator
module mux4to1(
    input [1:0] sel,
    input [7:0] a, b, c, d,
    output reg [7:0] out
);
    always_comb begin
        case (sel)
            2'b00: out = a;
            2'b01: out = b;
            2'b10: out = c;
            2'b11: out = d;
            default: out = 8'b0;
        endcase
    end
endmodule
```

### 3.3 Generate

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator
module adder_tree #(parameter WIDTH = 8, parameter LEVELS = 3)(
    input [WIDTH-1:0] in [0:(1<<LEVELS)-1],
    output [WIDTH-1:0] out
);
    genvar i;
    generate
        for (i = 0; i < LEVELS; i++) begin : gen_level
            // Generate logic here
        end
    endgenerate
endmodule
```

### 3.4 Function

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator
module alu(
    input [7:0] a, b,
    input [2:0] op,
    output reg [7:0] result
);
    function [7:0] alu_op(
        input [7:0] a, b,
        input [2:0] op
    );
        case (op)
            3'd0: alu_op = a + b;
            3'd1: alu_op = a - b;
            3'd2: alu_op = a & b;
            3'd3: alu_op = a | b;
            3'd4: alu_op = a ^ b;
            default: alu_op = 0;
        endcase
    endfunction

    always_comb begin
        result = alu_op(a, b, op);
    end
endmodule
```

### 3.5 Package

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator
package my_pkg;
    localparam WIDTH = 8;
    typedef enum logic [1:0] {IDLE, RUN, DONE} state_t;
endpackage

module top;
    import my_pkg::*;
    state_t state;
endmodule
```

---

## 4. Fitur yang Perlu Hati-hati

### 4.1 Blocking vs Non-blocking

```systemverilog
// ✅ Benar: always_comb pakai blocking
always_comb begin
    out = a + b;  // blocking OK
end

// ✅ Benar: always_ff pakai non-blocking
always_ff @(posedge clk) begin
    out <= a + b;  // non-blocking OK
end

// ❌ Salah: mixing blocking/non-blocking
always_ff @(posedge clk) begin
    out = a + b;  // ERROR: blocking in sequential
end
```

### 4.2 Latch Inference

```systemverilog
// ⚠️ Mivon: ok, Verilator: warns
always_comb begin
    if (en)
        out = data;
    // Missing else → latch inference
end

// ✅ Benar: complete if-else
always_comb begin
    if (en)
        out = data;
    else
        out = 0;  // no latch
end
```

### 4.3 Incomplete Sensitivity List

```systemverilog
// ❌ Mivon: ok (manual), Verilator: error
always @(a) begin
    out = a + b;  // b not in sensitivity list
end

// ✅ Benar: use always_comb
always_comb begin
    out = a + b;  // auto sensitivity
end
```

### 4.4 Mixed-width Operations

```systemverilog
// ⚠️ Mivon: wraps, Verilator: truncates
wire [7:0] a = 8'hFF;
wire [3:0] b = a;  // Truncated to 4'hF

// ✅ Explicit width
wire [7:0] a = 8'hFF;
wire [7:0] b = {4'b0, a[3:0]};  // Explicit
```

---

## 5. Perbandingan Mivon vs Verilator

| Aspek | Mivon | Verilator |
|-------|-------|-----------|
| **Tujuan** | Behavioral simulation | Linting + synthesis |
| **Kecepatan** | 1x (interpreted) | 100-1000x (compiled) |
| **4-state** | ✅ Full (X/Z) | ❌ 2-state only |
| **Timing** | ✅ Full (#delay, events) | ❌ Cycle-accurate only |
| **Testbench** | ✅ Full | ❌ RTL only |
| **UVM** | ✅ Basic | ❌ |
| **DPI-C** | ✅ Import | ✅ Import + Export |
| **Coverage** | ✅ Covergroup | ⚠️ Functional only |
| **Assertions** | ✅ Immediate + concurrent | ✅ Immediate only |
| **Error checking** | ⚠️ Partial | ✅ Excellent |
| **Lint rules** | ❌ | ✅ Built-in |
| **Code output** | AST (interpreted) | C++ (compiled) |

---

## 6. Tips Transisi

### Dari Mivon ke Verilator:
1. Hapus `#delay` dan `@(event)` (kecuali `@(posedge clk)`)
2. Ganti `always` → `always_ff`/`always_comb`
3. Hapus `$display`/`$finish` (testbench only)
4. Hapus `fork`/`join` (testbench only)
5. Ganti `initial` → testbench module
6. Periksa sensitivity list (`always_comb` auto-sensitivity)

### Dari Verilator ke Mivon:
1. Tambah `#delay` untuk timing control
2. Tambah `initial` block untuk stimulus
3. Tambah `$display`/`$finish` untuk monitoring
4. Tambah `fork`/`join` untuk concurrent testbench
5. Gunakan `force`/`release` untuk debug
6. Gunakan 4-state (X/Z) untuk propagation analysis

---

## 7. Contoh Testbench yang Kompatibel

```systemverilog
// ✅ Kompatibel dengan Mivon + Verilator (testbench)
module tb_counter;
    reg clk;
    reg rst_n;
    wire [3:0] count;

    counter uut(
        .clk(clk),
        .rst_n(rst_n),
        .count(count)
    );

    initial begin
        clk = 0;
        rst_n = 0;
        #5 rst_n = 1;
        #100 $finish;
    end

    always #5 clk = ~clk;

    // Mivon: uses $display; Verilator: uses lint
    // initial $monitor("time=%0t count=%h", $time, count);
endmodule
```

---

## 8. Daftar Verilator Directives (untuk Referensi)

```systemverilog
// Verilator-specific directives (tidak didukung Mivon)
// verilator lint_off WIDTH
// verilator lint_on WIDTH
// verilator coverage_off
// verilator coverage_on
// verilator inline_module
// verilator no_inline_module
```

---

## Ringkasan Kompatibilitas

| Kategori | Kompatibilitas |
|----------|---------------|
| RTL Synthesizable | ✅ ~90% kompatibel |
| Sequential Logic | ✅ ~95% kompatibel |
| Combinational Logic | ✅ ~95% kompatibel |
| Generate | ✅ ~90% kompatibel |
| Function/Task | ✅ ~80% kompatibel |
| Package | ✅ ~90% kompatibel |
| Interface | ✅ ~85% kompatibel |
| Assertion | ✅ ~70% kompatibel |
| Testbench | ❌ ~20% kompatibel |
| UVM | ❌ ~10% kompatibel |

**Kesimpulan:** Mivon cocok untuk simulasi behavioral RTL yang juga bisa di-lint oleh Verilator. Untuk testbench, Mivon memiliki fitur lengkap yang tidak tersedia di Verilator.

---

## 9. Hasil Sweep `examples/mv/` (11 Okt 2026)

Seluruh output `mgen` (36 file `.sv`) diuji terhadap iverilog `-g2012` dan
verilator `--lint-only --timing`. **Nol bug emisi baru** — semua penolakan
terpetakan ke limitasi tool yang sudah terdokumentasi di `MIVON-HDL.md`:

**iverilog bersih (20 file):** `always_latch`, `case_qualifiers`, `compound`,
`counter`, `disable_label`, `fib`, `force_release`, `fork_ctrl`,
`forkjoin`, `for_step`, `func_local` (warning static-init saja),
`gen_case` (warning sensivitas saja), `nets` (`wire`/`tri` OK),
`param_counter`, `prefix_incdec`, `queue`, `traffic`, `type_param`,
`wait_example`, `wire_assign`.

**iverilog menolak (limit tool):** concurrent `assert`/`assume`/`cover
property` (`assume_demo`, `cover_demo`, `escape_hatch`, `tb_counter`),
`bind` (`bind_check`), `covergroup` (`cover`), `case inside`
(`case_inside`), `unique if` (`unique_if`), named event `-> ev`
(`dowhile` — eksekusi), interface port (`axi_interface`), array-assign
(`array_lit`, `dsl_demo`, `multidim`), constraint class (`class_model`),
named function args `.factor(3)` (`features_new`), constant bit-select
`sorry` (`cast`), static-init warning (`func_local`).

**verilator bersih (27 file)** termasuk `axi_interface`, `case_inside`,
`unique_if`, `tb_counter`, `assume_demo`, `cover_demo`.

**verilator menolak (limit tool):** `covergroup` + coverage event
(`cover` — `UNSUPPORTED`), `wand`/`wor` (`nets` — `UNSUPPORTED`;
`wire`/`tri*` lolos), `disable` lintas-branch (`disable_label` —
non-standar, pakai `stop fork`), `-> ev` (`dowhile`/`dsl_demo` — Internal
Error `V3Delayed`), rekursi `$unit`-function (`fib` — Internal Error
`V3Scope`), `func_local` (Internal Error `V3Scope` + `IMPLICITSTATIC`
warning). Warning benign: `WIDTHEXPAND` (`counter`), `CASEINCOMPLETE`
(`traffic`), `IMPLICIT` bind-scope (`bind_check`).

---

*Panduan ini dibuat berdasarkan Mivon v0.2.9 dan Verilator 5.x (18 Juli 2026)*
