//! Codegen — module/program & isinya: port, signal/reg/const/use, blok logika
//! (seq/comb/always/latch/initial/final), instansi, generate, func/task.
//! 1 file = 1 tanggung jawab.

use super::expr::{emit_expr, emit_type};
use super::{emit_signal_decl, emit_signal_decl_multi, for_inc, line};
use crate::ast::*;

/// Emit `module` atau `program` (testbench) — struktur badan sama, hanya
/// keyword pembuka/penutup yang beda. `iface_names` = nama interface.
pub(crate) fn emit_module_kw(out: &mut String, m: &Module, kw: &str, iface_names: &[&str]) {
    // header module
    let mut head = format!("{kw} {} ", m.name);
    if !m.params.is_empty() {
        head.push_str("#(\n");
        let params: Vec<String> = m
            .params
            .iter()
            .map(|p| {
                let ty = if p.type_default.is_some()
                    || matches!(&p.ty, Some(MvType::Named(s, ..)) if s == "type")
                {
                    // F32: type param (marker `T : type = ...` ATAU bentuk kata
                    // kunci `type T = ...` — ty=None, type_default terisi).
                    format!(
                        "parameter type {} = {}",
                        p.name,
                        p.type_default.as_ref().map(emit_type).unwrap_or_default()
                    )
                } else if let Some(t) = &p.ty {
                    format!(
                        "parameter {} {} = {}",
                        emit_type(t),
                        p.name,
                        p.default.as_ref().map(emit_expr).unwrap_or_default()
                    )
                } else {
                    format!(
                        "parameter {} = {}",
                        p.name,
                        p.default.as_ref().map(emit_expr).unwrap_or_default()
                    )
                };
                format!("    {ty}")
            })
            .collect();
        head.push_str(&params.join(",\n"));
        head.push_str("\n) ");
    }
    head.push_str("(\n");
    // Nilai awal untuk `sig`/`reg` yang namanya sama dengan port output.
    // Deklarasi ganda di SV invalid, jadi reg di-skip — TAPI nilai
    // inisialisasinya tidak boleh hilang (`reg a : Addr = 16'h2A` +
    // `out a : Addr` →_port `output Addr a = 16'h2A`, sah LRM 1800 §6.8.2).
    // Hanya untuk arah out/inout — input port tak boleh diinisialisasi.
    let mut port_init: std::collections::HashMap<&str, &Expr> = std::collections::HashMap::new();
    for item in &m.items {
        let (names, init) = match item {
            MItem::Sig { names, init, .. }
            | MItem::Reg { names, init, .. }
            | MItem::Wire { names, init, .. } => (names, init),
            _ => continue,
        };
        if let Some(init) = init {
            for n in names {
                port_init.insert(n.as_str(), init);
            }
        }
    }

    // ports
    let mut port_lines: Vec<String> = Vec::new();
    for item in &m.items {
        if let MItem::Port(p) = item {
            let is_iface =
                matches!(&p.ty, MvType::Named(n, ..) if iface_names.contains(&n.as_str()));
            for n in &p.names {
                if is_iface {
                    // Port interface: `axi_lite axi_if` — tanpa arah (F26)
                    let ty = emit_type(&p.ty);
                    port_lines.push(format!("    {ty} {n}"));
                } else {
                    let dir = match p.dir {
                        Dir::In => "input",
                        Dir::Out => "output",
                        Dir::Inout => "inout",
                    };
                    let init_s = if p.dir == Dir::In {
                        String::new()
                    } else {
                        port_init
                            .get(n.as_str())
                            .map(|e| format!(" = {}", emit_expr(e)))
                            .unwrap_or_default()
                    };
                    port_lines.push(format!(
                        "    {dir:<7}{}{init_s}",
                        emit_signal_decl(&p.ty, n)
                    ));
                }
            }
        }
    }
    // Tanpa port: `module tb;` — bukan `module tb (\n\n);` yang sah SV
    // tapi tidak terbaca manusia (prinsip desain #4/#5).
    if port_lines.is_empty() {
        let mut h = head;
        // buang sisa `(\n` yang ditambahkan sebelum daftar port
        if h.ends_with("(\n") {
            h.truncate(h.len() - 2);
        }
        let h = h.trim_end().to_string();
        line(out, 0, &format!("{h};"));
    } else {
        head.push_str(&port_lines.join(",\n"));
        head.push_str("\n);");
        line(out, 0, &head);
    }

    // Kumpulkan nama port (agar sig/reg dengan nama port tidak dideklarasi ulang)
    let port_names: Vec<String> = m
        .items
        .iter()
        .filter_map(|i| match i {
            MItem::Port(p) => Some(p.names.clone()),
            _ => None,
        })
        .flatten()
        .collect();

    // deklarasi + blok + instansiasi — pertahankan urutan penulisan
    let mut labels = super::GenLabels::new();
    for item in &m.items {
        match item {
            MItem::Port(_) => {}
            MItem::Sig {
                names, ty, init, ..
            } => {
                let fresh: Vec<String> = names
                    .iter()
                    .filter(|n| !port_names.contains(n))
                    .cloned()
                    .collect();
                if !fresh.is_empty() {
                    // F28: `sig x : iface` = instance interface → emit dgn paren
                    // kosong (`axi_lite bus();`).
                    let is_iface = match ty {
                        MvType::Named(n, ..) => iface_names.contains(&n.as_str()),
                        _ => false,
                    };
                    if is_iface {
                        for nm in &fresh {
                            line(out, 1, &format!("{} {}();", emit_type(ty), nm));
                        }
                    } else {
                        let init_s = super::emit_init(init);
                        line(
                            out,
                            1,
                            &format!("{}{};", emit_signal_decl_multi(ty, &fresh), init_s),
                        );
                    }
                }
            }
            MItem::Reg {
                names, ty, init, ..
            } => {
                let fresh: Vec<String> = names
                    .iter()
                    .filter(|n| !port_names.contains(n))
                    .cloned()
                    .collect();
                if !fresh.is_empty() {
                    let init_s = super::emit_init(init);
                    line(
                        out,
                        1,
                        &format!(
                            "{}{};",
                            fresh
                                .iter()
                                .map(|nm| emit_signal_decl(ty, nm))
                                .collect::<Vec<_>>()
                                .join(", "),
                            init_s
                        ),
                    );
                }
            }
            MItem::Wire {
                net, names, ty, init, ..
            } => {
                let fresh: Vec<String> = names
                    .iter()
                    .filter(|n| !port_names.contains(n))
                    .cloned()
                    .collect();
                if !fresh.is_empty() {
                    let init_s = super::emit_init(init);
                    line(
                        out,
                        1,
                        &format!("{}{};", super::emit_wire_decl_multi(net.as_str(), ty, &fresh), init_s),
                    );
                }
            }
            MItem::Assign { lhs, rhs, .. } => {
                line(out, 1, &format!("assign {} = {};", emit_expr(lhs), emit_expr(rhs)));
            }
            MItem::Const {
                name, ty, value, ..
            } => {
                let ty_s = ty
                    .as_ref()
                    .map(|t| format!("{} ", emit_type(t)))
                    .unwrap_or_default();
                line(
                    out,
                    1,
                    &format!("localparam {ty_s}{name} = {};", emit_expr(value)),
                );
            }
            // `use` di-backend SV di-emit di SCOPE FILE (lihat
            // `codegen::mod::file_scope_imports`) — port list module
            // di-resolve di enclosing scope, bukan body (LRM 1800 §23.2.1.2).
            MItem::Use { .. } => {}
            // typedef lokal module juga di-hoist ke SCOPE FILE (lihat
            // `codegen::mod::file_scope_typedefs`) karena bisa dipakai di
            // port list module yang sama.
            MItem::Typedef(_) => {}
            MItem::AssertProperty(raw) => {
                line(out, 1, &format!("assert property {raw};"));
            }
            MItem::AssumeProperty(raw) => {
                line(out, 1, &format!("assume property {raw};"));
            }
            MItem::CoverProperty(raw) => {
                line(out, 1, &format!("cover property {raw};"));
            }
            MItem::Seq(spec, body) => {
                line(out, 0, "");
                line(out, 1, "// ── logika sekuensial ──");
                emit_seq(out, 1, spec, body);
            }
            MItem::Comb(body) => {
                line(out, 0, "");
                line(out, 1, "// ── logika kombinasional ──");
                line(out, 1, "always_comb begin");
                super::stmt::emit_body(out, 2, body);
                line(out, 1, "end");
            }
            MItem::Always(body) => {
                line(out, 0, "");
                line(out, 1, "// ── always ──");
                line(out, 1, "always begin");
                super::stmt::emit_body(out, 2, body);
                line(out, 1, "end");
            }
            MItem::Latch(body) => {
                line(out, 0, "");
                line(out, 1, "// ── latch ──");
                line(out, 1, "always_latch begin");
                super::stmt::emit_body(out, 2, body);
                line(out, 1, "end");
            }
            MItem::Initial(body) => {
                line(out, 0, "");
                line(out, 1, "// ── initial ──");
                line(out, 1, "initial begin");
                super::stmt::emit_body(out, 2, body);
                line(out, 1, "end");
            }
            MItem::Final(body) => {
                line(out, 0, "");
                line(out, 1, "// ── final ──");
                line(out, 1, "final begin");
                super::stmt::emit_body(out, 2, body);
                line(out, 1, "end");
            }
            MItem::Inst {
                module,
                name,
                dims,
                params,
                conns,
                ..
            } => {
                line(out, 0, "");
                emit_inst(out, 1, module, name, dims, params, conns);
            }
            MItem::Bind {
                target,
                module,
                name,
                dims,
                params,
                conns,
                ..
            } => {
                line(out, 0, "");
                emit_bind(out, 1, target, module, name, dims, params, conns);
            }
            MItem::GenFor {
                var,
                from,
                to,
                step,
                body,
            } => {
                let lbl = labels.uniq(&format!("gen_{var}"));
                line(out, 0, "");
                line(out, 0, "generate");
                line(
                    out,
                    1,
                    &format!(
                        "for (genvar {var} = {}; {var} < {}; {var} = {}) begin : {lbl}",
                        emit_expr(from),
                        emit_expr(to),
                        for_inc(var, step.as_ref())
                    ),
                );
                for item in body {
                    emit_module_item_at(out, 2, item, iface_names, &mut labels);
                }
                line(out, 1, "end");
                line(out, 0, "endgenerate");
            }
            MItem::GenIf { cond, then, els } => {
                let lbl = labels.uniq("gen_cond");
                let lbl_else = labels.uniq("gen_cond_else");
                line(out, 0, "");
                line(out, 0, "generate");
                line(out, 1, &format!("if ({}) begin : {lbl}", emit_expr(cond)));
                for item in then {
                    emit_module_item_at(out, 2, item, iface_names, &mut labels);
                }
                line(out, 1, "end");
                if !els.is_empty() {
                    line(out, 1, &format!("else begin : {lbl_else}"));
                    for item in els {
                        emit_module_item_at(out, 2, item, iface_names, &mut labels);
                    }
                    line(out, 1, "end");
                }
                line(out, 0, "endgenerate");
            }
            MItem::GenCase { expr, items, default, kind } => {
                // F79: generate case (LRM 1800 §27.5) — cabang ber-label unik.
                line(out, 0, "");
                line(out, 0, "generate");
                line(out, 1, &format!("{kind} ({})", emit_expr(expr)));
                for (labels_, body) in items {
                    let lbl = labels.uniq("gen_case");
                    let ls: Vec<String> = labels_.iter().map(emit_expr).collect();
                    line(out, 1, &format!("{}: begin : {lbl}", ls.join(", ")));
                    for item in body {
                        emit_module_item_at(out, 2, item, iface_names, &mut labels);
                    }
                    line(out, 1, "end");
                }
                if !default.is_empty() {
                    let lbl = labels.uniq("gen_case_else");
                    line(out, 1, &format!("default: begin : {lbl}"));
                    for item in default {
                        emit_module_item_at(out, 2, item, iface_names, &mut labels);
                    }
                    line(out, 1, "end");
                }
                line(out, 1, "endcase");
                line(out, 0, "endgenerate");
            }
            MItem::Func(f) => emit_func(out, f),
            MItem::Task(t) => emit_task(out, t),
        }
    }
    line(out, 0, &format!("end{kw}"));
}

/// Emit satu item module di indentasi tertentu — dipakai di level module
/// maupun di dalam blok generate. `labels` = uniquifier label generate
/// milik module ini (shared, biar dua blok tak menabrakkan label SV).
pub(crate) fn emit_module_item_at(
    out: &mut String,
    indent: usize,
    item: &MItem,
    iface_names: &[&str],
    labels: &mut super::GenLabels,
) {
    match item {
        MItem::Port(_) => {}
        MItem::Sig {
            names, ty, init, ..
        } => {
            let is_iface = match ty {
                MvType::Named(n, ..) => iface_names.contains(&n.as_str()),
                _ => false,
            };
            if is_iface {
                for nm in names {
                    line(out, indent, &format!("{} {}();", emit_type(ty), nm));
                }
            } else {
                let init_s = super::emit_init(init);
                line(
                    out,
                    indent,
                    &format!("{}{};", super::emit_signal_decl_multi(ty, names), init_s),
                );
            }
        }
        MItem::Reg {
            names, ty, init, ..
        } => {
            let init_s = super::emit_init(init);
            line(
                out,
                indent,
                &format!("{}{};", super::emit_signal_decl_multi(ty, names), init_s),
            );
        }
        MItem::Wire {
            net, names, ty, init, ..
        } => {
            let init_s = super::emit_init(init);
            line(
                out,
                indent,
                &format!("{}{};", super::emit_wire_decl_multi(net.as_str(), ty, names), init_s),
            );
        }
        MItem::Assign { lhs, rhs, .. } => {
            line(out, indent, &format!("assign {} = {};", emit_expr(lhs), emit_expr(rhs)));
        }
        MItem::Const {
            name, ty, value, ..
        } => {
            let ty_s = ty
                .as_ref()
                .map(|t| format!("{} ", emit_type(t)))
                .unwrap_or_default();
            line(
                out,
                indent,
                &format!("localparam {ty_s}{name} = {};", emit_expr(value)),
            );
        }
        // `use` di-backend SV di-emit di SCOPE FILE (lihat
        // `codegen::mod::file_scope_imports`).
        MItem::Use { .. } => {}
        // typedef lokal module di-hoist ke scope file (lihat
        // `codegen::mod::file_scope_typedefs`).
        MItem::Typedef(_) => {}
        MItem::AssertProperty(raw) => {
            // Concurrent assertion = module item (LRM 1800 §14), bukan
            // statement prosedural — di-emit di body module apa adanya.
            line(out, indent, &format!("assert property {raw};"));
        }
        MItem::AssumeProperty(raw) => {
            // Mirror `assert property`: asumsi concurrent = module item.
            line(out, indent, &format!("assume property {raw};"));
        }
        MItem::CoverProperty(raw) => {
            // Mirror `assert property`: cover concurrent = module item.
            line(out, indent, &format!("cover property {raw};"));
        }
        MItem::Seq(spec, body) => emit_seq(out, indent, spec, body),
        MItem::Comb(body) => {
            line(out, indent, "always_comb begin");
            super::stmt::emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        MItem::Always(body) => {
            line(out, indent, "always begin");
            super::stmt::emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        MItem::Latch(body) => {
            line(out, indent, "always_latch begin");
            super::stmt::emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        MItem::Initial(body) => {
            line(out, indent, "initial begin");
            super::stmt::emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        MItem::Final(body) => {
            line(out, indent, "final begin");
            super::stmt::emit_body(out, indent + 1, body);
            line(out, indent, "end");
        }
        MItem::Inst {
            module,
            name,
            dims,
            params,
            conns,
            ..
        } => {
            emit_inst(out, indent, module, name, dims, params, conns);
        }
        MItem::Bind {
            target,
            module,
            name,
            dims,
            params,
            conns,
            ..
        } => {
            emit_bind(out, indent, target, module, name, dims, params, conns);
        }
        MItem::GenFor {
            var,
            from,
            to,
            step,
            body,
        } => {
            line(out, indent, "generate");
            line(
                out,
                indent + 1,
                &format!(
                    "for (genvar {var} = {}; {var} < {}; {var} = {}) begin : {}",
                    emit_expr(from),
                    emit_expr(to),
                    for_inc(var, step.as_ref()),
                    labels.uniq(&format!("gen_{var}")),
                ),
            );
            for i in body {
                emit_module_item_at(out, indent + 2, i, iface_names, labels);
            }
            line(out, indent + 1, "end");
            line(out, indent, "endgenerate");
        }
        MItem::GenCase { expr, items, default, kind } => {
            // F79: generate case (LRM 1800 §27.5) — tiap cabang dibungkus
            // `begin : <label unik>` (label deterministik via GenLabels).
            line(out, indent, "generate");
            line(out, indent + 1, &format!("{kind} ({})", emit_expr(expr)));
            for (labels_, body) in items {
                let lbl = labels.uniq("gen_case");
                let ls: Vec<String> = labels_.iter().map(emit_expr).collect();
                line(out, indent + 1, &format!("{}: begin : {lbl}", ls.join(", ")));
                for i in body {
                    emit_module_item_at(out, indent + 2, i, iface_names, labels);
                }
                line(out, indent + 1, "end");
            }
            if !default.is_empty() {
                let lbl = labels.uniq("gen_case_else");
                line(out, indent + 1, &format!("default: begin : {lbl}"));
                for i in default {
                    emit_module_item_at(out, indent + 2, i, iface_names, labels);
                }
                line(out, indent + 1, "end");
            }
            line(out, indent + 1, "endcase");
            line(out, indent, "endgenerate");
        }
        MItem::GenIf { cond, then, els } => {
            let lbl = labels.uniq("gen_cond");
            let lbl_else = labels.uniq("gen_cond_else");
            line(out, indent, "generate");
            line(
                out,
                indent + 1,
                &format!("if ({}) begin : {lbl}", emit_expr(cond)),
            );
            for i in then {
                emit_module_item_at(out, indent + 2, i, iface_names, labels);
            }
            line(out, indent + 1, "end");
            if !els.is_empty() {
                line(out, indent + 1, &format!("else begin : {lbl_else}"));
                for i in els {
                    emit_module_item_at(out, indent + 2, i, iface_names, labels);
                }
                line(out, indent + 1, "end");
            }
            line(out, indent, "endgenerate");
        }
        MItem::Func(f) => emit_func(out, f),
        MItem::Task(t) => emit_task(out, t),
    }
}

pub(crate) fn emit_seq(out: &mut String, indent: usize, spec: &SeqSpec, body: &Stmt) {
    let edge = if spec.neg_edge { "negedge" } else { "posedge" };
    let mut event = format!("@({edge} {}", spec.clk);
    if let Some((rname, active_low, sync)) = &spec.reset {
        if !sync {
            let re = if *active_low { "negedge" } else { "posedge" };
            event.push_str(&format!(" or {re} {rname}"));
        }
    }
    event.push(')');
    line(out, indent, &format!("always_ff {event} begin"));
    super::stmt::emit_body(out, indent + 1, body);
    line(out, indent, "end");
}

pub(crate) fn emit_inst(
    out: &mut String,
    indent: usize,
    module: &str,
    name: &str,
    dims: &Option<Expr>,
    params: &[(String, Expr)],
    conns: &[Conn],
) {
    let dims_s = dims
        .as_ref()
        .map(|d| format!("[{}]", emit_expr(d)))
        .unwrap_or_default();
    let mut head = module.to_string();
    if !params.is_empty() {
        // positional `#(8)` (nama kosong) & named `#(.DEPTH(4))` — bisa campur
        // (SV mengizinkan positional dulu, lalu named).
        let ps: Vec<String> = params
            .iter()
            .map(|(n, e)| {
                if n.is_empty() {
                    emit_expr(e)
                } else {
                    format!(".{n}({})", emit_expr(e))
                }
            })
            .collect();
        head.push_str(&format!(" #({})", ps.join(", ")));
    }
    head.push_str(&format!(" {name}{dims_s}"));
    emit_inst_conns(out, indent, &head, conns);
}

/// F78: `bind <target> <module> [#(params)] <name>[dims] [(conns)];`
/// (LRM 1800 §23.11). Head sama seperti `inst` dengan prefix target.
pub(crate) fn emit_bind(
    out: &mut String,
    indent: usize,
    target: &str,
    module: &str,
    name: &str,
    dims: &Option<Expr>,
    params: &[(String, Expr)],
    conns: &[Conn],
) {
    let dims_s = dims
        .as_ref()
        .map(|d| format!("[{}]", emit_expr(d)))
        .unwrap_or_default();
    let mut head = format!("bind {target} {module}");
    if !params.is_empty() {
        let ps: Vec<String> = params
            .iter()
            .map(|(n, e)| {
                if n.is_empty() {
                    emit_expr(e)
                } else {
                    format!(".{n}({})", emit_expr(e))
                }
            })
            .collect();
        head.push_str(&format!(" #({})", ps.join(", ")));
    }
    head.push_str(&format!(" {name}{dims_s}"));
    emit_inst_conns(out, indent, &head, conns);
}

fn emit_inst_conns(out: &mut String, indent: usize, head: &str, conns: &[Conn]) {
    if conns.is_empty() {
        line(out, indent, &format!("{head};"));
        return;
    }
    let named: Vec<&Conn> = conns
        .iter()
        .filter(|c| matches!(c, Conn::Named { .. }))
        .collect();
    let max_len = named
        .iter()
        .map(|c| match c {
            Conn::Named { port, .. } => port.len(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    line(out, indent, &format!("{head} ("));
    let n = conns.len();
    for (i, c) in conns.iter().enumerate() {
        let sep = if i + 1 == n { "" } else { "," };
        match c {
            Conn::Named { port, expr } => {
                let e = expr.as_ref().map(emit_expr).unwrap_or_else(|| port.clone());
                line(
                    out,
                    indent + 1,
                    &format!(".{port}{}({e}){sep}", " ".repeat(max_len - port.len() + 1)),
                );
            }
            Conn::Positional(e) => {
                line(
                    out,
                    indent + 1,
                    &format!("{}({}){sep}", " ".repeat(max_len), emit_expr(e)),
                );
            }
        }
    }
    line(out, indent, ");");
}

pub(crate) fn emit_func(out: &mut String, f: &MFunc) {
    let ret = f
        .ret
        .as_ref()
        .map(emit_type)
        .unwrap_or_else(|| "void".into());
    let args = emit_args(&f.args, false);
    line(
        out,
        0,
        &format!("function {ret} {}({});", f.name, args.join(", ")),
    );
    for s in &f.body {
        super::stmt::emit_stmt(out, 1, s);
    }
    line(out, 0, "endfunction");
}

pub(crate) fn emit_task(out: &mut String, t: &MTask) {
    let args = emit_args(&t.args, true);
    line(out, 0, &format!("task {}({});", t.name, args.join(", ")));
    for s in &t.body {
        super::stmt::emit_stmt(out, 1, s);
    }
    line(out, 0, "endtask");
}

/// Format daftar argumen `(nama, tipe, arah, default)` — dipakai
/// emit_func/emit_task/emit_class (DRY). `default_inout`: arah default saat
/// tidak ditulis. Default arg → `input int b = 4`.
pub(crate) fn emit_args(
    args: &[(String, MvType, Option<Dir>, Option<Expr>)],
    default_inout: bool,
) -> Vec<String> {
    args.iter()
        .map(|(n, t, d, dflt)| {
            let dir = match d {
                Some(Dir::In) => "input",
                Some(Dir::Out) => "output",
                Some(Dir::Inout) => "inout",
                None if default_inout => "inout",
                None => "input",
            };
            let dflt_s = dflt
                .as_ref()
                .map(|e| format!(" = {}", emit_expr(e)))
                .unwrap_or_default();
            format!("{dir} {}{dflt_s}", emit_signal_decl(t, n))
        })
        .collect()
}
