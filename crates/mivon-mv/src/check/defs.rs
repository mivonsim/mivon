//! Check — tipe level file/package: typedef (alias/struct/union/enum) &
//! interface validate. 1 file = 1 tanggung jawab.

use super::expr::check_type;
use super::{err_at, new_scope, Ctx};
use crate::ast::*;
use crate::MvError;
use std::collections::HashSet;

pub(crate) fn check_typedef<'a>(td: &'a Typedef, ctx: &'a Ctx<'a>) -> Result<(), MvError> {
    match td {
        Typedef::Alias { ty, .. } => check_type(ty, ctx, 0),
        Typedef::Struct { name, fields, .. } | Typedef::Union { name, fields, .. } => {
            let mut fseen = HashSet::new();
            for f in fields {
                for n in &f.names {
                    if !fseen.insert(n.as_str()) {
                        return Err(err_at(
                            f.line,
                            f.col,
                            "E2007",
                            format!("field '{n}' dideklarasikan dua kali di struct/union '{name}'"),
                        ));
                    }
                }
                check_type(&f.ty, ctx, 0)?;
            }
            Ok(())
        }
        Typedef::Enum {
            name,
            width,
            members,
            ..
        } => {
            let scope = new_scope(ctx, name);
            if let Some(w) = width {
                super::expr::check_expr(w, ctx, &scope, 0)?;
            }
            let mut mseen = HashSet::new();
            for m in members {
                if !mseen.insert(m.name.as_str()) {
                    return Err(err_at(
                        m.line,
                        m.col,
                        "E2007",
                        format!(
                            "member '{}' dideklarasikan dua kali di enum '{name}'",
                            m.name
                        ),
                    ));
                }
                if let Some(v) = &m.value {
                    super::expr::check_expr(v, ctx, &scope, 0)?;
                }
            }
            Ok(())
        }
    }
}

/// Validasi interface: duplikat signal (port+sig), tipe dikenal (E2005),
/// dan setiap modport hanya merujuk signal yang ada (E2001).
pub(crate) fn check_interface<'a>(i: &'a Interface, ctx: &'a Ctx<'a>) -> Result<(), MvError> {
    let mut sig_names: HashSet<&'a str> = HashSet::new();
    let mut port_names: HashSet<&'a str> = HashSet::new();

    // port + sig sama-sama signal interface (lebar/nama namespace sama)
    for p in &i.ports {
        check_type(&p.ty, ctx, 0)?;
        for n in &p.names {
            if !port_names.insert(n.as_str()) {
                return Err(err_at(
                    p.line,
                    p.col,
                    "E2007",
                    format!(
                        "port '{}' dideklarasikan dua kali di interface '{}'",
                        n, i.name
                    ),
                ));
            }
            sig_names.insert(n.as_str());
        }
    }
    for (names, ty, line, col) in &i.sigs {
        check_type(ty, ctx, 0)?;
        for n in names {
            if !sig_names.insert(n.as_str()) {
                return Err(err_at(
                    *line,
                    *col,
                    "E2007",
                    format!(
                        "signal '{}' dideklarasikan dua kali di interface '{}'",
                        n, i.name
                    ),
                ));
            }
        }
    }

    // modport: nama unik + hanya merujuk signal yang dideklarasikan
    let mut mp_seen = HashSet::new();
    for mp in &i.modports {
        if !mp_seen.insert(mp.name.as_str()) {
            return Err(err_at(
                mp.line,
                mp.col,
                "E2007",
                format!(
                    "modport '{}' dideklarasikan dua kali di interface '{}'",
                    mp.name, i.name
                ),
            ));
        }
        // E2001: modport KOSONG (`modport e {}` tanpa item arah) — parser
        // menerima, tapi SV yang di-emit `modport e()` TIDAK sah (LRM §25.9:
        // modport butuh minimal satu simpul arah) → hasil transpile tak
        // parseable (fuzz bug_6365eafcd317). Error di level .mv.
        if mp.dirs.is_empty() {
            return Err(err_at(
                mp.line,
                mp.col,
                "E2001",
                format!(
                    "modport '{}' kosong — minimal satu item arah (in/out/inout) diperlukan di interface '{}'",
                    mp.name, i.name
                ),
            ));
        }
        for (_, names) in &mp.dirs {
            for n in names {
                if !sig_names.contains(n.as_str()) {
                    return Err(err_at(
                        mp.line,
                        mp.col,
                        "E2001",
                        format!(
                            "modport '{}' merujuk signal '{}' yang tidak ada di interface '{}'",
                            mp.name, n, i.name
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}
