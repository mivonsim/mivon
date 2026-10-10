//! Check — module: param, port, declarasi pass-1, statement pass-2,
//! instansiasi (koneksi port F29, override param F31/F32), generate.
//! 1 file = 1 tanggung jawab.

use super::{err_at, expr::check_expr, expr::check_type_scope, new_scope, Ctx, Scope};
use crate::ast::*;
use crate::MvError;
use std::collections::HashSet;

pub(crate) fn check_module<'a>(m: &'a Module, ctx: &'a Ctx<'a>) -> Result<(), MvError> {
    let mut port_names: HashSet<&'a str> = HashSet::new();
    let mut decl_names: HashSet<&'a str> = HashSet::new();
    let mut scope = new_scope(ctx, &m.name);

    // ── parameter: duplikat (E2007) + fold nilai + validasi tipe ──
    for p in &m.params {
        if scope.params.contains_key(p.name.as_str()) {
            return Err(err_at(
                p.line,
                p.col,
                "E2007",
                format!(
                    "parameter '{}' dideklarasikan dua kali di module '{}'",
                    p.name, m.name
                ),
            ));
        }
        // F32: type param — marker `Named("type")` (`T : type = ...`) ATAU
        // bentuk kata kunci `type T = ...` (ty=None, type_default terisi).
        let is_tp =
            p.type_default.is_some() || matches!(&p.ty, Some(MvType::Named(s, ..)) if s == "type");
        if is_tp {
            if let Some(td) = &p.type_default {
                check_type_scope(td, ctx, None, 0)?;
                scope.type_params.insert(p.name.as_str(), Some(td));
            } else {
                scope.type_params.insert(p.name.as_str(), None);
            }
        } else {
            if let Some(t) = &p.ty {
                check_type_scope(t, ctx, None, 0)?;
            }
            if let Some(v) = p
                .default
                .as_ref()
                .and_then(|e| super::expr::fold_const(e, &scope.params, 0))
            {
                scope.params.insert(p.name.as_str(), v);
            }
        }
    }

    // nama function/task module → terlihat sebagai ident
    for item in &m.items {
        match item {
            MItem::Func(f) => {
                scope.funcs.insert(f.name.as_str());
                super::insert_arity_pub(
                    &mut scope.func_arity,
                    f.name.as_str(),
                    super::arity_of_pub(&f.args),
                );
            }
            MItem::Task(t) => {
                scope.funcs.insert(t.name.as_str());
                super::insert_arity_pub(
                    &mut scope.func_arity,
                    t.name.as_str(),
                    super::arity_of_pub(&t.args),
                );
            }
            _ => {}
        }
    }

    // ── pass 1: kumpulkan deklarasi, deteksi duplikat (E2007), validasi tipe (E2005) ──
    for item in &m.items {
        match item {
            MItem::Port(p) => {
                let iface_ty = is_iface_type(&p.ty, ctx);
                for n in &p.names {
                    if !port_names.insert(n.as_str()) {
                        return Err(err_at(
                            p.line,
                            p.col,
                            "E2007",
                            format!("port '{n}' dideklarasikan dua kali di module '{}'", m.name),
                        ));
                    }
                    // F26: port interface tidak punya arah drive (field-nya
                    // diatur via modport) — jangan daftar ke env.ports agar
                    // E2003 tidak terpicu saat drive `axi_if.awready`.
                    if !iface_ty {
                        scope.env.ports.insert(n.as_str(), p.dir);
                    }
                }
                check_type_scope(&p.ty, ctx, Some(&scope), 0)?;
                for n in &p.names {
                    scope.sigs.insert(n.as_str());
                    scope.types.insert(n.as_str(), &p.ty);
                    scope.nonconsts.insert(n.as_str());
                }
            }
            MItem::Sig {
                names,
                ty,
                line,
                col,
                ..
            }
            | MItem::Reg {
                names,
                ty,
                line,
                col,
                ..
            }
            | MItem::Wire {
                names,
                ty,
                line,
                col,
                ..
            } => {
                check_type_scope(ty, ctx, Some(&scope), 0)?;
                for n in names {
                    if !decl_names.insert(n.as_str()) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2007",
                            format!(
                                "sinyal '{n}' dideklarasikan dua kali di module '{}'",
                                m.name
                            ),
                        ));
                    }
                    if scope.params.contains_key(n.as_str()) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2007",
                            format!(
                                "sinyal '{n}' bentrok dengan parameter di module '{}'",
                                m.name
                            ),
                        ));
                    }
                }
                for n in names {
                    scope.sigs.insert(n.as_str());
                    scope.types.insert(n.as_str(), ty);
                    scope.nonconsts.insert(n.as_str());
                }
            }
            MItem::Const {
                name,
                ty,
                value,
                line,
                col,
                ..
            } => {
                if let Some(t) = ty {
                    check_type_scope(t, ctx, Some(&scope), 0)?;
                }
                if !decl_names.insert(name.as_str()) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2007",
                        format!(
                            "konstanta '{name}' dideklarasikan dua kali di module '{}'",
                            m.name
                        ),
                    ));
                }
                if scope.params.contains_key(name.as_str()) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2007",
                        format!(
                            "konstanta '{name}' bentrok dengan parameter di module '{}'",
                            m.name
                        ),
                    ));
                }
                // Konstanta module ikut masuk `scope.params` (setelah di-fold)
                // supaya lebarnya bisa dipakai di `logic[C-1:0]`, `for i in
                // 0..C`, dll. Sebelumnya hanya nama yang didaftarkan tanpa
                // nilai → semua cek lebar lewat konstanta mati diam-diam.
                check_expr(value, ctx, &scope, 0)?;
                if let Some(v) = super::expr::fold_const(value, &scope.params, 0) {
                    scope.params.insert(name.as_str(), v);
                } else {
                    // Nilai tak ter-fold (mis. merujuk sinyal) — nama ini
                    // NON-konstan untuk fold kondisi generate (F80).
                    scope.nonconsts.insert(name.as_str());
                }
                scope.local_consts.insert(name.as_str());
                scope.sigs.insert(name.as_str());
            }
            MItem::Initial(_) | MItem::Final(_) => {}
            // typedef lokal module — daftarkan nama tipe + validasi isi
            MItem::Typedef(td) => {
                super::defs::check_typedef(td, ctx)?;
                let n = super::td_name(td);
                if scope.local_types.contains_key(n) {
                    return Err(err_at(
                        super::td_pos(td).0,
                        super::td_pos(td).1,
                        "E2007",
                        format!("tipe '{n}' dideklarasikan dua kali di module '{}'", m.name),
                    ));
                }
                scope.local_types.insert(n, td);
                // enum lokal: member dikenal sbg ident di module
                if let Typedef::Enum { members, .. } = td {
                    for mem in members {
                        scope.sigs.insert(mem.name.as_str());
                    }
                }
            }
            _ => {}
        }
    }

    // ── pass 2: statement (seq/comb/always/latch/initial/final/inst/gen/…) ──
    // E2003 hanya di DUT (tanpa initial/final); TB boleh drive input port.
    let is_tb = m
        .items
        .iter()
        .any(|i| matches!(i, MItem::Initial(_) | MItem::Final(_)));
    for item in &m.items {
        check_module_item(item, ctx, &mut scope, is_tb)?;
    }
    Ok(())
}

/// True bila tipe adalah nama interface (F26) — port bertipe interface
/// di-emit tanpa arah dan field-nya bebas di-drive (sesuai modport).
pub(crate) fn is_iface_type(ty: &MvType, ctx: &Ctx) -> bool {
    matches!(ty, MvType::Named(n, ..) if ctx.interfaces.contains(n.as_str()))
}

/// F75: tipe yang sah untuk `wire` — net hanya untuk bit/logic (signed)
/// atau typedef/array-nya. `int/real/string/queue` ditolak E2005.
fn is_wireable(ty: &MvType) -> bool {
    match ty {
        MvType::Bit | MvType::Logic(_) => true,
        MvType::Signed(inner) => matches!(
            inner.as_ref(),
            MvType::Bit | MvType::Logic(_) | MvType::Named(..)
        ),
        MvType::Array(inner, _) => is_wireable(inner),
        MvType::Named(..) => true,
        _ => false,
    }
}

/// Item module (termasuk di dalam blok generate).
fn check_module_item<'a>(
    item: &'a MItem,
    ctx: &'a Ctx<'a>,
    scope: &mut Scope<'a>,
    is_tb: bool,
) -> Result<(), MvError> {
    match item {
        MItem::Port(_) => Ok(()),    // port tidak valid di dalam generate — abaikan
        MItem::Typedef(_) => Ok(()), // sudah divalidasi pass-1 module
        // Body SVA mentah: dilewati (konservatif, sama seperti
        // `Stmt::AssertProperty` di check/stmt.rs) — operator SVA bukan
        // ekspresi `.mv` yang bisa divalidasi.
        MItem::AssertProperty(_) | MItem::AssumeProperty(_) | MItem::CoverProperty(_) => Ok(()),
        MItem::Sig {
            names, ty, init, ..
        }
        | MItem::Reg {
            names, ty, init, ..
        } => {
            check_type_scope(ty, ctx, Some(scope), 0)?;
            if let Some(i) = init {
                check_expr(i, ctx, scope, 0)?;
            }
            for n in names {
                scope.sigs.insert(n.as_str());
                scope.types.insert(n.as_str(), ty);
                scope.nonconsts.insert(n.as_str());
            }
            Ok(())
        }
        MItem::Wire {
            names, ty, init, line, col, ..
        } => {
            check_type_scope(ty, ctx, Some(scope), 0)?;
            if !is_wireable(ty) {
                return Err(err_at(
                    *line,
                    *col,
                    "E2005",
                    format!(
                        "wire hanya mendukung bit/logic (signed) atau typedef/array-nya — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            if let Some(i) = init {
                check_expr(i, ctx, scope, 0)?;
            }
            for n in names {
                scope.sigs.insert(n.as_str());
                scope.types.insert(n.as_str(), ty);
                scope.nonconsts.insert(n.as_str());
            }
            Ok(())
        }
        MItem::Assign { lhs, rhs, line, col } => {
            // Continuous assign: blocking semantik, bukan di seq.
            // Aturan sama seperti blocking di comb: E2003 (input port hanya
            // boleh di TB), E2001 via check_expr, E2010 lvalue const, E2002 width.
            if !is_tb {
                if let Some(base) = super::stmt::base_ident(lhs) {
                    if let Some(Dir::In) = scope.env.ports.get(base) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2003",
                            format!(
                                "cannot drive input port '{base}' — di '{}'",
                                scope.env.mname
                            ),
                        ));
                    }
                }
            }
            check_expr(lhs, ctx, scope, 0)?;
            check_expr(rhs, ctx, scope, 0)?;
            super::stmt::check_lvalue_not_const(lhs, scope, *line, *col)?;
            let wl = super::expr::expr_width(lhs, ctx, scope, 0);
            let wr = super::expr::expr_width(rhs, ctx, scope, 0);
            if let (Some(l), Some(r)) = (wl, wr) {
                if r > l {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2002",
                        format!(
                            "lebar {r} bit ke sinyal {l}-bit '{}' — di '{}'",
                            super::stmt::describe_lhs(lhs),
                            scope.env.mname
                        ),
                    ));
                }
            }
            Ok(())
        }
        MItem::Const {
            name, ty, value, ..
        } => {
            if let Some(t) = ty {
                check_type_scope(t, ctx, Some(scope), 0)?;
            }
            check_expr(value, ctx, scope, 0)?;
            // Cerminkan pass-1: konstanta ter-fold ikut `params` (terlihat di
            // kondisi generate tersarang); sisanya NON-konstan (F80).
            if let Some(v) = super::expr::fold_const(value, &scope.params, 0) {
                scope.params.insert(name.as_str(), v);
            } else {
                scope.nonconsts.insert(name.as_str());
            }
            scope.sigs.insert(name.as_str());
            Ok(())
        }
        MItem::Use { .. } => Ok(()),
        MItem::Seq(spec, body) => {
            // F26: clock bisa `iface.clk` — validasi base ident-nya saja.
            let clk_base = spec.clk.split('.').next().unwrap_or(&spec.clk);
            if !scope.known(clk_base) {
                return Err(err_at(
                    spec.line,
                    spec.col,
                    "E2001",
                    format!(
                        "undefined signal '{}' (clock seq) — di module '{}'",
                        spec.clk, scope.env.mname
                    ),
                ));
            }
            if let Some((rname, _, _)) = &spec.reset {
                if !scope.known(rname) {
                    return Err(err_at(
                        spec.line,
                        spec.col,
                        "E2001",
                        format!(
                            "undefined signal '{rname}' (reset seq) — di module '{}'",
                            scope.env.mname
                        ),
                    ));
                }
            }
            super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Seq)
        }
        MItem::Comb(body) => super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Always),
        MItem::Always(body) => super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Always),
        MItem::Latch(body) => super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Always),
        MItem::Initial(body) => super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Tb),
        MItem::Final(body) => super::stmt::check_stmt(body, ctx, scope, super::BlockKind::Tb),
        MItem::Inst {
            module,
            name,
            dims,
            params,
            conns,
            line,
            col,
        }
        | MItem::Bind {
            target: _,
            module,
            name,
            dims,
            params,
            conns,
            line,
            col,
            ..
        } => {
            // F78: target hierarkis `bind` (`dut`, `top.u_mem`) SENGAJA tidak
            // divalidasi: urutan deklarasi instance tak tentu (bind bisa
            // mendahului `inst` targetnya) sehingga E2001 rawan false
            // positive. Module/param/port koneksi di bawah divalidasi penuh
            // bila module-nya dikenali — sama seperti `inst`.
            if let Some(d) = dims {
                check_expr(d, ctx, scope, 0)?;
            }
            // F31: validasi override parameter `#(.NAME(expr))` BILA target
            // dikenali. Nama eksternal dilewati (konservatif).
            let target_known =
                ctx.modules.contains(module.as_str()) || ctx.interfaces.contains(module.as_str());
            let kind = if ctx.interfaces.contains(module.as_str()) {
                "interface"
            } else {
                "module"
            };
            let pnames = ctx
                .module_params
                .get(module.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let tpnames = ctx
                .module_type_params
                .get(module.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let mut seen_params: HashSet<&str> = HashSet::new();
            for (pn, e) in params {
                // Parameter POSITIONAL `#(8)` (nama kosong dari parser) — hanya
                // validasi ekspresi; tak ada nama utk dicocokkan/duplicate.
                if pn.is_empty() {
                    check_expr(e, ctx, scope, 0)?;
                    continue;
                }
                // F32: override TYPE param (`.T(Word16)`) — nilai adalah TIPE.
                let tp_known = target_known && tpnames.iter().any(|s| s == pn);
                let tp_like = !target_known && super::expr::is_type_like(e, ctx);
                if tp_known || tp_like {
                    match e {
                        Expr::Ident(tn, _, _) => {
                            let t = MvType::Named(tn.clone(), 0, 0);
                            check_type_scope(&t, ctx, Some(scope), 0)?;
                        }
                        Expr::Scoped(pkg, item, ..) => {
                            let t = MvType::Named(format!("{}::{}", pkg, item), 0, 0);
                            check_type_scope(&t, ctx, Some(scope), 0)?;
                        }
                        other => {
                            if tp_known {
                                return Err(err_at(
                                    *line,
                                    *col,
                                    "E2005",
                                    format!(
                                        "nilai override type param '{}' harus nama tipe (dapatkan: {:?})",
                                        pn, other
                                    ),
                                ));
                            }
                            check_expr(e, ctx, scope, 0)?;
                        }
                    }
                    if !seen_params.insert(pn.as_str()) {
                        return Err(err_at(
                            *line,
                            *col,
                            "E2007",
                            format!(
                                "parameter '{}' di-override dua kali di {} '{}'",
                                pn, kind, module
                            ),
                        ));
                    }
                    continue;
                }
                check_expr(e, ctx, scope, 0)?;
                if target_known && !pnames.iter().any(|p| p == pn) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2001",
                        format!("parameter '{}' tidak ada di {} '{}'", pn, kind, module),
                    ));
                }
                if !seen_params.insert(pn.as_str()) {
                    return Err(err_at(
                        *line,
                        *col,
                        "E2007",
                        format!(
                            "parameter '{}' di-override dua kali di {} '{}'",
                            pn, kind, module
                        ),
                    ));
                }
            }
            // F29: validasi koneksi port BILA target dikenali.
            let ports = ctx
                .module_ports
                .get(module.as_str())
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let mut pos_count = 0usize;
            let mut seen_ports: HashSet<&str> = HashSet::new();
            for c in conns {
                match c {
                    Conn::Named { port, expr } => {
                        if let Some(e) = expr {
                            check_expr(e, ctx, scope, 0)?;
                        }
                        if target_known && !ports.iter().any(|p| p == port) {
                            return Err(err_at(
                                *line,
                                *col,
                                "E2001",
                                format!("port '{}' tidak ada di {} '{}'", port, kind, module),
                            ));
                        }
                        // F29 fix review: port dikoneksikan dua kali → E2007.
                        if !seen_ports.insert(port.as_str()) {
                            return Err(err_at(
                                *line,
                                *col,
                                "E2007",
                                format!(
                                    "port '{}' dikoneksikan dua kali di {} '{}'",
                                    port, kind, module
                                ),
                            ));
                        }
                    }
                    Conn::Positional(e) => {
                        pos_count += 1;
                        check_expr(e, ctx, scope, 0)?;
                    }
                }
            }
            if target_known && pos_count > ports.len() {
                return Err(err_at(
                    *line,
                    *col,
                    "E2001",
                    format!(
                        "terlalu banyak koneksi positional: {} koneksi untuk {} port di {} '{}'",
                        pos_count,
                        ports.len(),
                        kind,
                        module
                    ),
                ));
            }
            // nama instance bisa direferensikan (mis. `u_mem.data`) — tapi
            // NON-konstan untuk fold generate (F80).
            scope.sigs.insert(name.as_str());
            scope.nonconsts.insert(name.as_str());
            Ok(())
        }
        MItem::GenFor {
            var,
            from,
            to,
            step,
            body,
        } => {
            check_expr(from, ctx, scope, 0)?;
            check_expr(to, ctx, scope, 0)?;
            if let Some(s) = step {
                check_expr(s, ctx, scope, 0)?;
            }
            let mut inner = scope.clone();
            inner.sigs.insert(var.as_str());
            inner.genvars.insert(var.as_str());
            inner.nonconsts.insert(var.as_str());
            for it in body {
                check_module_item(it, ctx, &mut inner, is_tb)?;
            }
            Ok(())
        }
        MItem::GenIf {
            cond,
            then,
            els,
            line,
            col,
        } => {
            check_expr(cond, ctx, scope, 0)?;
            // F80: kondisi generate WAJIB konstan waktu-elaborasi (LRM 1800
            // §27.5) — parameter/konstanta/literal, bukan sinyal. Elaborator
            // hanya warning + ambil cabang pertama diam-diam.
            // Genvar tersubstitusi per iterasi oleh elaborator → E2014
            // dilewati bila kondisi merujuknya.
            let uses_genvar = scope.genvars.iter().any(|g| super::expr::expr_uses(cond, g));
            if !uses_genvar && super::expr::gen_const_value(cond, scope, ctx, 0).is_none() {
                return Err(err_at(
                    *line,
                    *col,
                    "E2014",
                    format!(
                        "kondisi generate-if harus konstan (parameter/konstanta) — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            let mut inner = scope.clone();
            for it in then {
                check_module_item(it, ctx, &mut inner, is_tb)?;
            }
            let mut inner2 = scope.clone();
            for it in els {
                check_module_item(it, ctx, &mut inner2, is_tb)?;
            }
            Ok(())
        }
        MItem::GenCase {
            expr,
            items,
            default,
            line,
            col,
            ..
        } => {
            // F79: generate case — label konstan divalidasi sebagai ekspresi
            // (E2001/E2005); konstness final ditegakkan elaborator SV.
            // Tiap cabang scope sendiri (pola GenIf): `sig` cabang-0 tak
            // terbaca di cabang-1 (cabang generate = scope terpisah).
            check_expr(expr, ctx, scope, 0)?;
            // F80: expr generate-case WAJIB konstan (LRM 1800 §27.5).
            // Genvar tersubstitusi per iterasi → E2014 dilewati bila
            // ekspresi merujuknya (lihat GenIf di atas).
            let uses_genvar = scope.genvars.iter().any(|g| super::expr::expr_uses(expr, g));
            if !uses_genvar && super::expr::gen_const_value(expr, scope, ctx, 0).is_none() {
                return Err(err_at(
                    *line,
                    *col,
                    "E2014",
                    format!(
                        "ekspresi generate-case harus konstan (parameter/konstanta) — di '{}'",
                        scope.env.mname
                    ),
                ));
            }
            for (labels, body) in items {
                for l in labels {
                    check_expr(l, ctx, scope, 0)?;
                }
                let mut inner = scope.clone();
                for it in body {
                    check_module_item(it, ctx, &mut inner, is_tb)?;
                }
            }
            let mut inner_d = scope.clone();
            for it in default {
                check_module_item(it, ctx, &mut inner_d, is_tb)?;
            }
            Ok(())
        }
        MItem::Func(f) => super::class::check_func(f, ctx, scope),
        MItem::Task(t) => super::class::check_task(t, ctx, scope),
    }
}
