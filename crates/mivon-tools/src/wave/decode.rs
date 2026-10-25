//! Subkomando `decode` — protokol-aware decode transaksi bus dari VCD
//! (WAV-16): `apb` / `axi4lite` / `ahb`.

use mivon_core::error::SimError;

use super::data::{parse_vcd, VcdData};
use super::util::{bits_val, err, find_signal, sample_at, timeline_of, union_times};

/// Satu transaksi ter-dekode dari waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedTx {
    /// nomor urut transaksi (1-based)
    pub index: usize,
    /// "WRITE" | "READ"
    pub kind: &'static str,
    pub start: u64,
    pub end: u64,
    pub addr: u64,
    /// wdata (write) atau rdata (read)
    pub data: u64,
    /// response AXI4-Lite (bresp/rresp), None utk APB/AHB
    pub resp: Option<u64>,
}

/// Decode APB: SETUP (psel=1, penable=0) → ACCESS (psel=1, penable=1),
/// selesai saat pready=1. pready tidak ada → diasumsikan selalu 1.
pub fn apb_decode(data: &VcdData) -> Result<Vec<DecodedTx>, SimError> {
    for n in ["psel", "penable", "pwrite", "paddr"] {
        if find_signal(data, n).is_none() {
            return Err(err(format!("decode apb: sinyal '{}' tidak ditemukan", n)));
        }
    }
    let tl = |n: &str| {
        find_signal(data, n)
            .map(|s| timeline_of(data, s))
            .unwrap_or_default()
    };
    let psel = tl("psel");
    let penable = tl("penable");
    let pwrite = tl("pwrite");
    let paddr = tl("paddr");
    let pwdata = tl("pwdata");
    let prdata = tl("prdata");
    // pready absent → selalu ready (AMBA: tanpa wait state).
    let pready = find_signal(data, "pready")
        .map(|s| timeline_of(data, s))
        .unwrap_or_else(|| vec![(0, "1".to_string())]);

    let times = union_times(&[&psel, &penable, &pready]);
    let mut out: Vec<DecodedTx> = Vec::new();
    // transaksi berjalan: (write, start, addr, wdata)
    let mut cur: Option<(bool, u64, u64, u64)> = None;

    for &t in &times {
        let sel = sample_at(&psel, t) == "1";
        let en = sample_at(&penable, t) == "1";
        let rdy = sample_at(&pready, t) == "1";

        if sel && !en {
            // fase SETUP — mulai transaksi baru
            cur = Some((
                sample_at(&pwrite, t) == "1",
                t,
                bits_val(&sample_at(&paddr, t)),
                bits_val(&sample_at(&pwdata, t)),
            ));
        } else if sel && en && rdy {
            // ACCESS dengan pready=1 — transaksi selesai
            if let Some((write, start, addr, wdata)) = cur.take() {
                let data_v = if write {
                    wdata
                } else {
                    bits_val(&sample_at(&prdata, t))
                };
                out.push(DecodedTx {
                    index: out.len() + 1,
                    kind: if write { "WRITE" } else { "READ" },
                    start,
                    end: t,
                    addr,
                    data: data_v,
                    resp: None,
                });
            }
        }
    }
    Ok(out)
}

/// Handshake valid/ready pada waktu t.
fn handshake(vld: &[(u64, String)], rdy: &[(u64, String)], t: u64) -> bool {
    sample_at(vld, t) == "1" && sample_at(rdy, t) == "1"
}

/// Decode AXI4-Lite: write (aw/w/b) + read (ar/r) channel handshake valid/ready.
pub fn axi4lite_decode(data: &VcdData) -> Result<Vec<DecodedTx>, SimError> {
    let tl = |n: &str| {
        find_signal(data, n)
            .map(|s| timeline_of(data, s))
            .unwrap_or_default()
    };
    let has_write = find_signal(data, "awvalid").is_some();
    let has_read = find_signal(data, "arvalid").is_some();
    if !has_write && !has_read {
        return Err(err(
            "decode axi4lite: tidak ada channel write (awvalid) / read (arvalid)",
        ));
    }

    let mut txs: Vec<DecodedTx> = Vec::new();

    if has_write {
        let awv = tl("awvalid");
        let awr = tl("awready");
        let wv = tl("wvalid");
        let wr = tl("wready");
        let bv = tl("bvalid");
        let br = tl("bready");
        let awaddr = tl("awaddr");
        let wdata = tl("wdata");
        let bresp = tl("bresp");

        let mut addr: Option<(u64, u64)> = None; // (start, addr)
        let mut wdat = 0u64;
        let mut have_wdata = false;
        let mut times = union_times(&[&awv, &awr, &wv, &wr, &bv, &br]);
        times.sort_unstable();

        for &t in &times {
            if handshake(&awv, &awr, t) {
                addr = Some((t, bits_val(&sample_at(&awaddr, t))));
                have_wdata = false;
            }
            if handshake(&wv, &wr, t) && !have_wdata {
                wdat = bits_val(&sample_at(&wdata, t));
                have_wdata = true;
            }
            if handshake(&bv, &br, t) {
                if let Some((start, a)) = addr.take() {
                    txs.push(DecodedTx {
                        index: 0,
                        kind: "WRITE",
                        start,
                        end: t,
                        addr: a,
                        data: wdat,
                        resp: Some(bits_val(&sample_at(&bresp, t))),
                    });
                }
            }
        }
    }

    if has_read {
        let arv = tl("arvalid");
        let arr = tl("arready");
        let rv = tl("rvalid");
        let rr = tl("rready");
        let araddr = tl("araddr");
        let rdata = tl("rdata");
        let rresp = tl("rresp");

        let mut addr: Option<(u64, u64)> = None;
        let mut times = union_times(&[&arv, &arr, &rv, &rr]);
        times.sort_unstable();

        for &t in &times {
            if handshake(&arv, &arr, t) {
                addr = Some((t, bits_val(&sample_at(&araddr, t))));
            }
            if handshake(&rv, &rr, t) {
                if let Some((start, a)) = addr.take() {
                    txs.push(DecodedTx {
                        index: 0,
                        kind: "READ",
                        start,
                        end: t,
                        addr: a,
                        data: bits_val(&sample_at(&rdata, t)),
                        resp: Some(bits_val(&sample_at(&rresp, t))),
                    });
                }
            }
        }
    }

    txs.sort_by_key(|tx| tx.end);
    for (i, tx) in txs.iter_mut().enumerate() {
        tx.index = i + 1;
    }
    Ok(txs)
}

/// Decode AHB-Lite single transfer: alamat phase saat htrans=NONSEQ("10")/
/// SEQ("11") dengan hready=1; data phase selesai pada hready=1 berikutnya.
pub fn ahb_decode(data: &VcdData) -> Result<Vec<DecodedTx>, SimError> {
    for n in ["htrans", "haddr", "hwrite", "hready"] {
        if find_signal(data, n).is_none() {
            return Err(err(format!("decode ahb: sinyal '{}' tidak ditemukan", n)));
        }
    }
    let tl = |n: &str| {
        find_signal(data, n)
            .map(|s| timeline_of(data, s))
            .unwrap_or_default()
    };
    let htrans = tl("htrans");
    let haddr = tl("haddr");
    let hwrite = tl("hwrite");
    let hready = tl("hready");
    let hwdata = tl("hwdata");
    let hrdata = tl("hrdata");

    let is_addr_phase = |tr: &str| tr == "10" || tr == "11"; // NONSEQ | SEQ

    let times = union_times(&[&htrans, &hready]);
    let mut out: Vec<DecodedTx> = Vec::new();
    // transaksi menunggu data phase: (write, start, addr)
    let mut cur: Option<(bool, u64, u64)> = None;

    for &t in &times {
        let tr = sample_at(&htrans, t);
        let rdy = sample_at(&hready, t) == "1";

        match cur.take() {
            None => {
                if is_addr_phase(&tr) && rdy {
                    cur = Some((
                        sample_at(&hwrite, t) == "1",
                        t,
                        bits_val(&sample_at(&haddr, t)),
                    ));
                }
            }
            Some((write, start, addr)) => {
                if rdy {
                    let data_v = if write {
                        bits_val(&sample_at(&hwdata, t))
                    } else {
                        bits_val(&sample_at(&hrdata, t))
                    };
                    out.push(DecodedTx {
                        index: out.len() + 1,
                        kind: if write { "WRITE" } else { "READ" },
                        start,
                        end: t,
                        addr,
                        data: data_v,
                        resp: None,
                    });
                } else {
                    cur = Some((write, start, addr));
                }
                // Pipelining: address phase transfer baru bisa dimulai pada
                // cycle yang sama dengan data phase sebelumnya selesai.
                if rdy && is_addr_phase(&tr) {
                    cur = Some((
                        sample_at(&hwrite, t) == "1",
                        t,
                        bits_val(&sample_at(&haddr, t)),
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// Subkomando `decode` — dekode transaksi protokol bus dari VCD (WAV-16).
pub(crate) fn decode(input: &str, proto: &str) -> Result<(), SimError> {
    let data = parse_vcd(input)?;
    let p = proto.to_ascii_lowercase().replace('_', "");
    let txs = match p.as_str() {
        "apb" => apb_decode(&data)?,
        "axi4lite" | "axi-lite" | "axi4-lite" | "axilite" => axi4lite_decode(&data)?,
        "ahb" | "ahblite" | "ahb-lite" => ahb_decode(&data)?,
        other => {
            return Err(err(format!(
                "protokol tidak dikenal: '{}' (pilihan: apb | axi4lite | ahb)",
                other
            )))
        }
    };

    println!(
        "  decode: {} (proto={}, {} sinyal, max_time={})",
        input,
        proto,
        data.signals.len(),
        data.max_time
    );
    for tx in &txs {
        let dir = if tx.kind == "WRITE" { "wdata" } else { "rdata" };
        match tx.resp {
            Some(r) => println!(
                "  #{:<3} {:<5} [{:>5}..{:>5}] addr=0x{:x} {}=0x{:x} resp=0x{:x}",
                tx.index, tx.kind, tx.start, tx.end, tx.addr, dir, tx.data, r
            ),
            None => println!(
                "  #{:<3} {:<5} [{:>5}..{:>5}] addr=0x{:x} {}=0x{:x}",
                tx.index, tx.kind, tx.start, tx.end, tx.addr, dir, tx.data
            ),
        }
    }
    let nw = txs.iter().filter(|t| t.kind == "WRITE").count();
    let nr = txs.len() - nw;
    println!(
        "  total: {} transaksi ({} write, {} read)",
        txs.len(),
        nw,
        nr
    );
    Ok(())
}
