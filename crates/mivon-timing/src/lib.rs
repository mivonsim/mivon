//! Mivon Timing & Area (SYNTHESIS.md §15/§16 — phase 5).
//!
//! Static timing analysis (STA) pada netlist hasil synthesis:
//!
//! ```text
//! netlist (tech/generic) ──analyze──► TimingReport (arrival/required/slack)
//!        │                                 WNS / TNS / critical path
//!        └──constraint .mcs──► Constraint  (clock period, IO delay, ...)
//! ```
//!
//! - `constraint` — parser `.mcs` (Mivon Constraint Specification):
//!   `clock clk { period = 10ns; }`, `input_delay 2ns;`, `output_delay 2ns;`,
//!   `max_fanout 32;`, `false_path { from = rst; }`,
//!   `multicycle_path 2 { from = reg_a; to = reg_b; }`.
//! - `timing` — propagation arrival dari startpoint (input port / FF-Q) ke
//!   endpoint (FF-D / output port), slack = required − arrival. WNS = slack
//!   terkecil, TNS = jumlah slack negatif, critical path dilaporkan.
//! - `area` — estimasi area dari resource (LUT/FF/CARRY4/...) dalam satuan
//!   unit area (bukan gate count ASIC — itu `mivon-tech`/Liberty fase 6-7).

pub mod area;
pub mod constraint;
pub mod timing;

pub use area::{estimate_area, render_area_report, AreaReport};
pub use constraint::{load_constraints, parse_constraints, ClockSpec, Constraint, PathSpec};
pub use timing::{analyze, render_timing_report, TimingOptions, TimingReport};

/// Versi library.
pub const VERSION: &str = "0.1.0";

#[cfg(test)]
mod lib_tests {
    use super::*;
    use mivon_core::intern::Symbol;
    use mivon_netlist::{
        CellInstance, CellKind, Netlist, PinConn, Port, PortDir,
    };

    /// Helper: bangun netlist in (port) -> LUT u0 -> mid -> FF u1(D) -> q,
    /// dengan clock clk dan output out = mid (buffer).
    fn sta_netlist() -> Netlist {
        let mut nl = Netlist::new(Symbol::intern("top"));
        let in_net = nl.add_net(Symbol::intern("in"), 1);
        let mid_net = nl.add_net(Symbol::intern("mid"), 1);
        let clk_net = nl.add_net(Symbol::intern("clk"), 1);
        let q_net = nl.add_net(Symbol::intern("q"), 1);
        let out_net = nl.add_net(Symbol::intern("out"), 1);

        nl.ports.push(Port {
            name: Symbol::intern("in"),
            dir: PortDir::Input,
            width: 1,
        });
        nl.ports.push(Port {
            name: Symbol::intern("clk"),
            dir: PortDir::Input,
            width: 1,
        });
        nl.ports.push(Port {
            name: Symbol::intern("out"),
            dir: PortDir::Output,
            width: 1,
        });

        // LUT u0: in -> mid
        let mut lut = CellInstance::new(Symbol::intern("u0"), CellKind::Lut { init: 0x1 }, 1);
        lut.inputs = vec![PinConn::whole(in_net, "i0")];
        lut.outputs = vec![PinConn::whole(mid_net, "o")];

        // FF u1: clk->c, mid->d, output q
        let mut ff = CellInstance::new(Symbol::intern("u1"), CellKind::Dff, 1);
        ff.inputs = vec![
            PinConn::whole(clk_net, "c"),
            PinConn::whole(mid_net, "d"),
        ];
        ff.outputs = vec![PinConn::whole(q_net, "q")];

        // Buffer buf: mid -> out
        let mut buf = CellInstance::new(Symbol::intern("buf"), CellKind::Buffer, 1);
        buf.inputs = vec![PinConn::whole(mid_net, "i")];
        buf.outputs = vec![PinConn::whole(out_net, "o")];

        nl.add_cell(lut);
        nl.add_cell(ff);
        nl.add_cell(buf);
        nl
    }

    #[test]
    fn lib_constraint_period_and_multiplier() {
        let c = Constraint::default();
        assert!((c.period_ns() - 10.0).abs() < 1e-9, "default periode 10 ns");

        let mut c2 = Constraint::default();
        c2.clocks.push(ClockSpec {
            name: "clk".into(),
            period_ns: 5.0,
        });
        assert!((c2.period_ns() - 5.0).abs() < 1e-9);

        // false path: from="rst" cocok semua (wildcard)
        let mut c_fp = Constraint::default();
        c_fp.false_paths.push(PathSpec {
            from: Some("rst".into()),
            to: None,
            cycles: 1,
        });
        assert!(c_fp.is_false_path("rst", "anything"));
        assert!(!c_fp.is_false_path("a", "b"));
        assert!(!c.is_false_path("rst", "anything"));

        // multicycle 2 dari reg_a ke reg_b
        let mut c3 = Constraint::default();
        c3.multicycle_paths.push(PathSpec {
            from: Some("reg_a".into()),
            to: Some("reg_b".into()),
            cycles: 2,
        });
        assert_eq!(c3.cycle_multiplier("reg_a", "reg_b"), 2);
        assert_eq!(c3.cycle_multiplier("reg_a", "reg_c"), 1);
        assert_eq!(c3.cycle_multiplier("reg_x", "reg_b"), 1);
    }

    #[test]
    fn lib_estimate_area_multi_cell() {
        let mut nl = Netlist::new(Symbol::intern("top"));
        let a = nl.add_net(Symbol::intern("a"), 1);
        let b = nl.add_net(Symbol::intern("b"), 1);
        let clk = nl.add_net(Symbol::intern("clk"), 1);
        let q = nl.add_net(Symbol::intern("q"), 1);
        let mut lut = CellInstance::new(Symbol::intern("u0"), CellKind::Lut { init: 0x1 }, 1);
        lut.inputs = vec![PinConn::whole(a, "i0")];
        lut.outputs = vec![PinConn::whole(b, "o")];
        let mut ff = CellInstance::new(Symbol::intern("ff"), CellKind::Dff, 1);
        ff.inputs = vec![
            PinConn::whole(clk, "c"),
            PinConn::whole(b, "d"),
        ];
        ff.outputs = vec![PinConn::whole(q, "q")];
        nl.add_cell(lut);
        nl.add_cell(ff);
        let r = estimate_area(&nl);
        assert_eq!(r.lut, 1);
        assert_eq!(r.ff, 1);
        // 1 LUT (1.0) + 1 FF bit (0.5) = 1.5
        assert!((r.area_units - 1.5).abs() < 1e-9);
        let report = render_area_report(&r);
        assert!(report.contains("1.50"));
    }

    #[test]
    fn lib_sta_wns_and_tns() {
        let nl = sta_netlist();
        let c = Constraint {
            input_delay_ns: 2.0,
            output_delay_ns: 2.0,
            ..Default::default()
        };
        let opts = TimingOptions {
            setup_ns: 0.2,
            clk_to_q_ns: 0.5,
        };
        let report = analyze(&nl, &c, &opts);

        // FF-D endpoint: arrival[mid] = 2.0(in) + 0.30(LUT) + 0.05*fanout(2)
        // = 2.40. required = 10.0 - 0.2 = 9.8, slack = 7.40.
        let ff_ep = report
            .endpoints
            .iter()
            .find(|e| e.kind == "ff")
            .expect("FF-D endpoint harus ada");
        assert!((ff_ep.arrival_ns - 2.40).abs() < 1e-6);
        assert!((ff_ep.required_ns - 9.8).abs() < 1e-6);
        assert!((ff_ep.slack_ns - 7.40).abs() < 1e-6);

        // Output port endpoint: arrival[out] = 2.40 + 0.10(BUF) + 0.05*0 =
        // 2.50. required = 10.0 - 2.0 = 8.0, slack = 5.50.
        let out_ep = report
            .endpoints
            .iter()
            .find(|e| e.kind == "out")
            .expect("output-port endpoint harus ada");
        assert!((out_ep.arrival_ns - 2.50).abs() < 1e-6);
        assert!((out_ep.required_ns - 8.0).abs() < 1e-6);
        assert!((out_ep.slack_ns - 5.50).abs() < 1e-6);

        // WNS = min(7.40, 5.50) = 5.50, TNS = 0
        assert!((report.wns_ns - 5.50).abs() < 1e-6, "wns={}", report.wns_ns);
        assert!((report.tns_ns - 0.0).abs() < 1e-6, "tns={}", report.tns_ns);

        let txt = render_timing_report(&report, "clk");
        assert!(txt.contains("WNS"));
        assert!(txt.contains("TNS"));
    }
}
