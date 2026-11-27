//! Circuit model: nets, DC solver and the bridge between the ESP32 and the devices.

use crate::i2cbus::SoftI2c;
use crate::{Device, PinView, PwmState, Rail, RmtItem, Stamp};
use esp32_soc::io::{IoBridge, PinDrive, SpiPins, NUM_GPIO};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};

pub const VDD: f64 = 3.3;
const R_OUT: f64 = 30.0; // ESP32 output driver resistance
const R_PULL: f64 = 45_000.0; // internal pull-up / pull-down
const G_LEAK: f64 = 1e-12;

/// A placed device and the nets its pins connect to.
pub struct Component {
    pub id: String,
    pub dev: Box<dyn Device>,
    pub nets: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct NetInfo {
    pub name: String,
    pub members: Vec<String>,
}

/// Log entry shown in the UI / console.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BoardEvent {
    pub time_ns: u64,
    pub source: String,
    pub text: String,
}

pub struct Board {
    pub comps: Vec<Component>,
    pub nets: Vec<NetInfo>,
    /// ESP32 pad -> net
    pub pad_net: [Option<usize>; NUM_GPIO],
    pub gnd: usize,
    pub v33: usize,
    pub v5: usize,
    /// net -> group (independent sub-circuit)
    group_of: Vec<usize>,
    groups: Vec<Vec<usize>>,
    /// components per group
    group_comps: Vec<Vec<usize>>,
    dirty: Vec<bool>,
    any_dirty: bool,
    pub volts: Vec<Option<f64>>,
    diode_i: HashMap<(usize, u32), f64>,
    /// capacitor charging states, keyed by (component, stamp tag)
    cap_dyn: HashMap<(usize, u32), CapDyn>,
    pub drive: PinDrive,
    pub pwm: [Option<PwmState>; NUM_GPIO],
    net_pwm: Vec<Option<PwmState>>,
    pub now: u64,
    next_poll: Option<u64>,
    /// next time a charging capacitor needs its group re-solved
    cap_next: Option<u64>,
    /// UART0 output (USB serial console)
    pub console_out: VecDeque<u8>,
    /// UART0 input queue (from the console)
    pub console_in: VecDeque<u8>,
    pub events: VecDeque<BoardEvent>,
    soft_i2c: Vec<SoftI2c>,
    /// Last seen chip-select level of each SPI device (true = high / deselected).
    spi_cs: Vec<bool>,
    /// total uart bytes per port (for UI)
    pub uart_counts: [u64; 3],
    /// logic-analyzer recording of ESP32 pad levels
    pub trace: PadTrace,
    /// bus monitor (transaction level)
    pub buslog: BusLog,
    pub solves: u64,
}

impl Board {
    /// Build a board from components and wires (pairs of "component.pin" endpoints).
    pub fn new(comps_in: Vec<(String, Box<dyn Device>)>, wires: &[(String, String)], pad_names: &dyn Fn(&str) -> Option<u8>) -> Result<Board, String> {
        // endpoint ids: "comp.pin"
        let mut ep_index: HashMap<String, usize> = HashMap::new();
        let mut endpoints: Vec<String> = Vec::new();
        let add_ep = |name: String, ep_index: &mut HashMap<String, usize>, endpoints: &mut Vec<String>| -> usize {
            if let Some(&i) = ep_index.get(&name) {
                return i;
            }
            let i = endpoints.len();
            endpoints.push(name.clone());
            ep_index.insert(name, i);
            i
        };
        // rails
        let gnd_ep = add_ep("GND".into(), &mut ep_index, &mut endpoints);
        let v33_ep = add_ep("3V3".into(), &mut ep_index, &mut endpoints);
        let v5_ep = add_ep("5V".into(), &mut ep_index, &mut endpoints);
        // ESP32 pads
        for g in 0..NUM_GPIO {
            add_ep(format!("esp32.GPIO{}", g), &mut ep_index, &mut endpoints);
        }
        let mut comp_pin_ep: Vec<Vec<usize>> = Vec::new();
        for (id, dev) in &comps_in {
            let mut v = Vec::new();
            for p in dev.pins() {
                v.push(add_ep(format!("{}.{}", id, p.name), &mut ep_index, &mut endpoints));
            }
            comp_pin_ep.push(v);
        }
        // resolve an endpoint name (with aliases) to an endpoint index
        let resolve = |raw: &str| -> Result<usize, String> {
            let s = raw.trim();
            let up = s.to_ascii_uppercase();
            if matches!(up.as_str(), "GND" | "0V" | "VSS" | "ESP32.GND" | "BOARD.GND") {
                return Ok(gnd_ep);
            }
            if matches!(up.as_str(), "3V3" | "3.3V" | "VCC" | "ESP32.3V3" | "BOARD.3V3" | "3V") {
                return Ok(v33_ep);
            }
            if matches!(up.as_str(), "5V" | "VIN" | "VBUS" | "ESP32.VIN" | "ESP32.5V" | "BOARD.VIN" | "BOARD.5V") {
                return Ok(v5_ep);
            }
            if let Some((c, p)) = s.split_once('.') {
                let cl = c.to_ascii_lowercase();
                if cl == "esp32" || cl == "board" || cl == "esp" || cl == "mcu" {
                    if let Some(g) = pad_names(p) {
                        return Ok(ep_index[&format!("esp32.GPIO{}", g)]);
                    }
                    return Err(format!("unknown ESP32 pin '{}'", p));
                }
                // exact match first, then case-insensitive
                if let Some(&i) = ep_index.get(s) {
                    return Ok(i);
                }
                for (k, &i) in ep_index.iter() {
                    if k.eq_ignore_ascii_case(s) {
                        return Ok(i);
                    }
                }
                return Err(format!("unknown pin '{}'", s));
            }
            // bare pad name like "GPIO5" / "D5"
            if let Some(g) = pad_names(s) {
                return Ok(ep_index[&format!("esp32.GPIO{}", g)]);
            }
            Err(format!("cannot resolve '{}'", s))
        };
        // union-find over endpoints
        let n = endpoints.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            let mut y = x;
            while p[y] != r {
                let nx = p[y];
                p[y] = r;
                y = nx;
            }
            r
        }
        for (a, b) in wires {
            let ia = resolve(a)?;
            let ib = resolve(b)?;
            let (ra, rb) = (find(&mut parent, ia), find(&mut parent, ib));
            if ra != rb {
                parent[ra] = rb;
            }
        }
        // power pins of devices named VCC/GND/etc. that are not wired explicitly are left floating.
        // nets
        let mut root_net: HashMap<usize, usize> = HashMap::new();
        let mut nets: Vec<NetInfo> = Vec::new();
        let mut ep_net = vec![0usize; n];
        for i in 0..n {
            let r = find(&mut parent, i);
            let ni = *root_net.entry(r).or_insert_with(|| {
                nets.push(NetInfo::default());
                nets.len() - 1
            });
            ep_net[i] = ni;
            nets[ni].members.push(endpoints[i].clone());
        }
        let gnd = ep_net[gnd_ep];
        let v33 = ep_net[v33_ep];
        let v5 = ep_net[v5_ep];
        if gnd == v33 || gnd == v5 || v33 == v5 {
            return Err("short circuit: supply rails are wired together".into());
        }
        for (i, net) in nets.iter_mut().enumerate() {
            net.name = if i == gnd {
                "GND".into()
            } else if i == v33 {
                "3V3".into()
            } else if i == v5 {
                "5V".into()
            } else {
                net.members.iter().find(|m| m.starts_with("esp32.")).cloned().unwrap_or_else(|| net.members[0].clone())
            };
        }
        let mut pad_net = [None; NUM_GPIO];
        for g in 0..NUM_GPIO {
            let ni = ep_net[ep_index[&format!("esp32.GPIO{}", g)]];
            // only consider the pad connected if something else is on the net
            if nets[ni].members.len() > 1 {
                pad_net[g] = Some(ni);
            }
        }
        let comps: Vec<Component> = comps_in
            .into_iter()
            .zip(comp_pin_ep)
            .map(|((id, dev), eps)| Component { id, dev, nets: eps.iter().map(|&e| ep_net[e]).collect() })
            .collect();
        let nn = nets.len();
        let mut b = Board {
            comps,
            nets,
            pad_net,
            gnd,
            v33,
            v5,
            group_of: vec![0; nn],
            groups: Vec::new(),
            group_comps: Vec::new(),
            dirty: Vec::new(),
            any_dirty: true,
            volts: vec![None; nn],
            diode_i: HashMap::new(),
            cap_dyn: HashMap::new(),
            drive: PinDrive::default(),
            pwm: [None; NUM_GPIO],
            net_pwm: vec![None; nn],
            now: 0,
            next_poll: Some(0),
            cap_next: None,
            trace: PadTrace::default(),
            buslog: BusLog::default(),
            console_out: VecDeque::new(),
            console_in: VecDeque::new(),
            events: VecDeque::new(),
            soft_i2c: Vec::new(),
            spi_cs: Vec::new(),
            uart_counts: [0; 3],
            solves: 0,
        };
        b.partition();
        b.setup_soft_i2c();
        b.solve_all();
        Ok(b)
    }

    fn is_rail(&self, net: usize) -> bool {
        net == self.gnd || net == self.v33 || net == self.v5
    }

    fn rail_volts(&self, net: usize) -> Option<f64> {
        if net == self.gnd {
            Some(0.0)
        } else if net == self.v33 {
            Some(VDD)
        } else if net == self.v5 {
            Some(5.0)
        } else {
            None
        }
    }

    /// Split the circuit into independent groups: all non-rail pins of a device share a group.
    fn partition(&mut self) {
        let nn = self.nets.len();
        let mut parent: Vec<usize> = (0..nn).collect();
        fn find(p: &mut Vec<usize>, x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            p[x] = r;
            r
        }
        for c in &self.comps {
            let mut first: Option<usize> = None;
            for &net in &c.nets {
                if self.is_rail(net) {
                    continue;
                }
                match first {
                    None => first = Some(net),
                    Some(f) => {
                        let (a, b) = (find(&mut parent, f), find(&mut parent, net));
                        if a != b {
                            parent[a] = b;
                        }
                    }
                }
            }
        }
        let mut gid: HashMap<usize, usize> = HashMap::new();
        self.groups.clear();
        for net in 0..nn {
            if self.is_rail(net) {
                self.group_of[net] = usize::MAX;
                continue;
            }
            let r = find(&mut parent, net);
            let g = *gid.entry(r).or_insert_with(|| {
                self.groups.push(Vec::new());
                self.groups.len() - 1
            });
            self.group_of[net] = g;
            self.groups[g].push(net);
        }
        self.group_comps = vec![Vec::new(); self.groups.len()];
        for (ci, c) in self.comps.iter().enumerate() {
            let mut gs: Vec<usize> = c.nets.iter().filter(|n| !self.is_rail(**n)).map(|n| self.group_of[*n]).collect();
            gs.sort();
            gs.dedup();
            if gs.is_empty() && c.nets.iter().filter(|n| self.is_rail(**n)).count() >= 2 {
                // wired only between supply rails (e.g. an LED straight across 5V/GND):
                // give it a node-less group so it still gets solved and notified
                self.groups.push(Vec::new());
                self.group_comps.push(vec![ci]);
                continue;
            }
            for g in gs {
                self.group_comps[g].push(ci);
            }
        }
        self.dirty = vec![true; self.groups.len()];
        self.any_dirty = true;
    }

    fn setup_soft_i2c(&mut self) {
        // one pin-level I2C decoder per distinct (SCL, SDA) net pair used by I2C devices
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for c in &self.comps {
            if let Some((scl, sda)) = c.dev.i2c_pins() {
                let p = (c.nets[scl], c.nets[sda]);
                if !pairs.contains(&p) {
                    pairs.push(p);
                }
            }
        }
        self.soft_i2c = pairs.into_iter().map(|(scl, sda)| SoftI2c::new(scl, sda)).collect();
    }

    fn mark_net(&mut self, net: usize) {
        if let Some(&g) = self.group_of.get(net) {
            if g != usize::MAX {
                self.dirty[g] = true;
                self.any_dirty = true;
            }
        }
    }

    pub fn mark_all(&mut self) {
        for d in self.dirty.iter_mut() {
            *d = true;
        }
        self.any_dirty = true;
    }

    fn solve_all(&mut self) {
        self.mark_all();
        self.solve_dirty();
    }

    /// Solve all dirty groups, then notify devices.  Devices that change their
    /// outputs in response trigger further passes (at most 8: they settle quickly).
    pub fn solve_dirty(&mut self) {
        for _ in 0..8 {
            if !self.any_dirty {
                return;
            }
            self.solve_pass();
        }
        self.any_dirty = false;
    }

    fn solve_pass(&mut self) {
        self.any_dirty = false;
        // rails
        for net in [self.gnd, self.v33, self.v5] {
            self.volts[net] = self.rail_volts(net);
        }
        let mut touched: Vec<usize> = Vec::new();
        for g in 0..self.groups.len() {
            if !self.dirty[g] {
                continue;
            }
            self.dirty[g] = false;
            self.solve_group(g);
            touched.push(g);
        }
        self.solves += 1;
        // notify devices of the touched groups
        let mut notified = vec![false; self.comps.len()];
        for g in touched {
            for &ci in &self.group_comps[g].clone() {
                if notified[ci] {
                    continue;
                }
                notified[ci] = true;
                self.notify(ci);
            }
        }
        // soft I2C decoders
        for k in 0..self.soft_i2c.len() {
            let (scl, sda) = (self.soft_i2c[k].scl, self.soft_i2c[k].sda);
            let lv = |v: Option<f64>| v.map(|x| x > 1.65).unwrap_or(true);
            let (l_scl, l_sda) = (lv(self.volts[scl]), lv(self.volts[sda]));
            let mut dec = std::mem::take(&mut self.soft_i2c[k]);
            let changed = dec.update(self, l_scl, l_sda);
            self.soft_i2c[k] = dec;
            if changed {
                self.mark_net(sda);
            }
        }
        self.record_pads();
        // devices that changed their outputs while being notified (solved in the next pass)
        for ci in 0..self.comps.len() {
            if self.comps[ci].dev.take_dirty() {
                for n in self.comps[ci].nets.clone() {
                    self.mark_net(n);
                }
            }
        }
    }

    fn notify(&mut self, ci: usize) {
        let nets = self.comps[ci].nets.clone();
        let volts: Vec<Option<f64>> = nets.iter().map(|&n| self.volts[n]).collect();
        let pwm: Vec<Option<PwmState>> = nets.iter().map(|&n| self.net_pwm[n]).collect();
        let di = &self.diode_i;
        let f = move |tag: u32| -> f64 { *di.get(&(ci, tag)).unwrap_or(&0.0) };
        let view = PinView { volts: &volts, diode_current: &f, pwm: &pwm };
        let now = self.now;
        self.comps[ci].dev.on_solved(now, &view);
        // chip-select edges (GPIO-driven CS) delimit SPI transactions
        if let Some((_, _, _, cs)) = self.comps[ci].dev.spi_pins() {
            if self.spi_cs.len() < self.comps.len() {
                self.spi_cs.resize(self.comps.len(), true);
            }
            let high = volts.get(cs).copied().flatten().map(|v| v > 1.65).unwrap_or(true);
            if high != self.spi_cs[ci] {
                self.spi_cs[ci] = high;
                self.comps[ci].dev.spi_select(!high);
            }
        }
        // the device may have scheduled timed behaviour
        self.next_poll = Some(now);
    }

    /// Nodal analysis of one group (with iterative diode states).
    ///
    /// Capacitors are transient elements: at the instant of a solve each one acts
    /// as a voltage source holding its present voltage.  Two more solves (cap
    /// open / shorted) give the Thevenin equivalent it sees, and its voltage then
    /// follows v(t) = Vth + (v0 - Vth)·e^(-t/RC) until the next event.  While a
    /// capacitor is still charging, the group is re-solved periodically
    /// (see `advance_caps`).
    fn solve_group(&mut self, g: usize) {
        let nets = self.groups[g].clone();
        let n = nets.len();
        let mut idx: HashMap<usize, usize> = HashMap::new();
        for (i, &net) in nets.iter().enumerate() {
            idx.insert(net, i);
        }
        let now = self.now;
        let mut els: Vec<El> = Vec::new();
        let mut caps: Vec<(usize, usize, f64, usize, u32)> = Vec::new();
        let mut driven = vec![false; n];
        let mut pwm_net: Vec<Option<PwmState>> = vec![None; n];
        // device stamps first: capacitors change how PWM outputs are modelled
        let mut dev_els: Vec<El> = Vec::new();
        let mut st: Vec<Stamp> = Vec::new();
        for &ci in &self.group_comps[g] {
            st.clear();
            self.comps[ci].dev.stamps(&mut st);
            let cn = &self.comps[ci].nets;
            for s in &st {
                match *s {
                    Stamp::Resistor { a, b, ohms } => dev_els.push(El::R(cn[a], cn[b], ohms.max(1e-3))),
                    Stamp::Source { a, volts, ohms } => dev_els.push(El::Src(cn[a], volts, ohms.max(1e-3))),
                    Stamp::Diode { a, k, vf, ron, tag } => dev_els.push(El::D(cn[a], cn[k], vf, ron.max(1e-3), ci, tag)),
                    Stamp::ToRail { a, rail, ohms } => {
                        let v = match rail {
                            Rail::Gnd => 0.0,
                            Rail::V3 => VDD,
                            Rail::V5 => 5.0,
                        };
                        dev_els.push(El::Src(cn[a], v, ohms.max(1e-3)));
                    }
                    Stamp::Capacitor { a, b, farads, tag } => {
                        if farads > 0.0 && cn[a] != cn[b] {
                            caps.push((cn[a], cn[b], farads, ci, tag));
                        }
                    }
                }
            }
        }
        // ESP32 pads
        for pad in 0..NUM_GPIO {
            let net = match self.pad_net[pad] {
                Some(x) => x,
                None => continue,
            };
            if !idx.contains_key(&net) {
                continue;
            }
            let bit = 1u64 << pad;
            let d = &self.drive;
            if pad == 25 || pad == 26 {
                if let Some(v) = d.dac[pad - 25] {
                    els.push(El::Src(net, v as f64, 1000.0));
                    continue;
                }
            }
            if let Some(p) = self.pwm[pad] {
                if p.running || p.duty > 0.0 {
                    if caps.is_empty() {
                        // model the high phase; consumers scale by duty
                        els.push(El::Src(net, if p.duty > 0.0 { VDD } else { 0.0 }, R_OUT));
                        pwm_net[idx[&net]] = Some(p);
                    } else {
                        // an RC network averages the PWM waveform
                        els.push(El::Src(net, VDD * p.duty.clamp(0.0, 1.0), R_OUT));
                    }
                    continue;
                }
            }
            if d.out_en & bit != 0 {
                let high = d.out_level & bit != 0;
                if d.open_drain & bit != 0 {
                    if !high {
                        els.push(El::Src(net, 0.0, R_OUT));
                    }
                } else {
                    els.push(El::Src(net, if high { VDD } else { 0.0 }, R_OUT));
                }
            }
            if d.pull_up & bit != 0 {
                els.push(El::Src(net, VDD, R_PULL));
            }
            if d.pull_down & bit != 0 {
                els.push(El::Src(net, 0.0, R_PULL));
            }
        }
        // bit-banged I2C targets pulling SDA low
        for dec in &self.soft_i2c {
            if dec.drive_low && idx.contains_key(&dec.sda) {
                els.push(El::Src(dec.sda, 0.0, 50.0));
            }
        }
        // I2C breakout boards carry pull-up resistors on SDA/SCL
        for &ci in &self.group_comps[g] {
            if let Some((scl, sda)) = self.comps[ci].dev.i2c_pins() {
                for p in [scl, sda] {
                    let net = self.comps[ci].nets[p];
                    if idx.contains_key(&net) {
                        els.push(El::Src(net, VDD, 10_000.0));
                    }
                }
            }
        }
        els.extend(dev_els);
        // which nodes are connected (through conductive paths) to something that sets a voltage?
        let local = |net: usize| idx.get(&net).copied();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut seed: Vec<usize> = Vec::new();
        let link = |a: usize, b: usize, adj: &mut Vec<Vec<usize>>, seed: &mut Vec<usize>| match (local(a), local(b)) {
            (Some(x), Some(y)) => {
                adj[x].push(y);
                adj[y].push(x);
            }
            (Some(x), None) => seed.push(x),
            (None, Some(y)) => seed.push(y),
            _ => {}
        };
        for e in &els {
            match *e {
                El::R(a, b, _) | El::D(a, b, _, _, _, _) | El::F(a, b, _, _) => link(a, b, &mut adj, &mut seed),
                El::Src(a, _, _) => {
                    if let Some(x) = local(a) {
                        seed.push(x);
                    }
                }
            }
        }
        for c in &caps {
            link(c.0, c.1, &mut adj, &mut seed);
        }
        let mut reach = vec![false; n];
        let mut stack = seed.clone();
        while let Some(x) = stack.pop() {
            if reach[x] {
                continue;
            }
            reach[x] = true;
            for &y in &adj[x] {
                if !reach[y] {
                    stack.push(y);
                }
            }
        }
        for (i, r) in reach.iter().enumerate() {
            driven[i] = *r;
        }
        let rails = (self.gnd, self.v33, self.v5);
        // capacitor voltages right now (continuous across the event)
        let cap_v: Vec<f64> = caps.iter().map(|c| self.cap_dyn.get(&(c.3, c.4)).map(|d| d.value_at(now)).unwrap_or(0.0)).collect();
        let with_caps = |mode: Option<(usize, bool)>| -> Vec<El> {
            // mode: None = every cap is a source at its voltage;
            // Some((k, open)) = cap k open (true) or shorted (false), others sources
            let mut v = els.clone();
            for (i, c) in caps.iter().enumerate() {
                match mode {
                    Some((k, true)) if k == i => {}
                    Some((k, false)) if k == i => v.push(El::F(c.0, c.1, 1.0 / CAP_ESR, 0.0)),
                    _ => v.push(El::F(c.0, c.1, 1.0 / CAP_ESR, cap_v[i])),
                }
            }
            v
        };
        let (sol, don) = solve_nonlinear(&with_caps(None), n, &local, rails);
        // Thevenin equivalent seen by each capacitor -> its charging curve
        let mut cap_updates = Vec::new();
        for (i, c) in caps.iter().enumerate() {
            let vd = |sol: &Vec<f64>| net_v(c.0, sol, &local, rails) - net_v(c.1, sol, &local, rails);
            let (s_open, _) = solve_nonlinear(&with_caps(Some((i, true))), n, &local, rails);
            let v_oc = vd(&s_open);
            let (s_short, _) = solve_nonlinear(&with_caps(Some((i, false))), n, &local, rails);
            // Rth from whichever of the two loaded solves (cap at its voltage / cap
            // shorted) draws more current: i = (Vth - v) / Rth
            let (vd_src, vd_sh) = (vd(&sol), vd(&s_short));
            let i_src = (vd_src - cap_v[i]) / CAP_ESR;
            let i_sh = vd_sh / CAP_ESR;
            let (dv, ii) = if i_src.abs() > i_sh.abs() { (v_oc - vd_src, i_src) } else { (v_oc - vd_sh, i_sh) };
            let r_th = if ii.abs() < 1e-12 || dv / ii <= 0.0 { f64::INFINITY } else { (dv / ii).max(1e-3) };
            let tau = r_th * c.2;
            cap_updates.push(((c.3, c.4), CapDyn { v0: cap_v[i], t0: now, v_th: v_oc, tau, group: g, v_solved: cap_v[i] }));
        }
        for (k, d) in cap_updates {
            self.cap_dyn.insert(k, d);
        }
        for (i, &net) in nets.iter().enumerate() {
            self.volts[net] = if driven[i] { Some(sol[i]) } else { None };
            self.net_pwm[net] = pwm_net[i];
        }
        // diode currents
        let mut di = 0;
        for e in &els {
            if let El::D(a, k, vf, ron, ci, tag) = *e {
                let va = self.volts.get(a).copied().flatten().or(self.rail_volts(a)).unwrap_or(0.0);
                let vk = self.volts.get(k).copied().flatten().or(self.rail_volts(k)).unwrap_or(0.0);
                let i = if don[di] { ((va - vk - vf) / ron).max(0.0) } else { 0.0 };
                self.diode_i.insert((ci, tag), i);
                di += 1;
            }
        }
    }

    /// Capacitors still charging: re-solve their groups when their voltage has
    /// moved noticeably, and return when the next check is due.
    fn advance_caps(&mut self, now: u64) -> Option<u64> {
        let mut next: Option<u64> = None;
        let mut groups = Vec::new();
        for d in self.cap_dyn.values_mut() {
            if !d.tau.is_finite() || d.tau <= 0.0 {
                continue;
            }
            let v = d.value_at(now);
            let remaining = (d.v_th - v).abs();
            if remaining < 0.5e-3 {
                if (d.v_solved - d.v_th).abs() >= 0.5e-3 {
                    // settled: one last solve at the final value
                    d.v0 = d.v_th;
                    d.t0 = now;
                    groups.push(d.group);
                }
                continue;
            }
            let step_v = ((d.v_th - d.v0).abs() * 0.01).max(2e-3);
            if (v - d.v_solved).abs() >= step_v {
                groups.push(d.group);
            }
            let dt = ((d.tau * 1e9) / 32.0).clamp(1_000.0, 20_000_000.0) as u64;
            next = Some(next.map_or(now + dt, |x: u64| x.min(now + dt)));
        }
        for g in groups {
            if g < self.dirty.len() {
                self.dirty[g] = true;
                self.any_dirty = true;
            }
        }
        next
    }

    /// Append pad level changes to the logic-analyzer trace.
    fn record_pads(&mut self) {
        if !self.trace.enabled {
            return;
        }
        for g in 0..NUM_GPIO {
            let Some(net) = self.pad_net[g] else { continue };
            let lv = self.volts[net].map(|v| v > 1.65);
            if lv != self.trace.last[g] {
                self.trace.last[g] = lv;
                self.trace.push(self.now, g as u8, lv);
            }
        }
    }

    /// Logic level on an ESP32 pad, if something drives its net.
    pub fn pad_level(&self, pad: u8) -> Option<bool> {
        let net = self.pad_net.get(pad as usize).copied().flatten()?;
        self.volts[net].map(|v| v > 1.65)
    }
    pub fn pad_volts(&self, pad: u8) -> Option<f64> {
        let net = self.pad_net.get(pad as usize).copied().flatten()?;
        self.volts[net]
    }
    pub fn net_level(&self, net: usize) -> Option<bool> {
        self.volts.get(net).copied().flatten().map(|v| v > 1.65)
    }

    /// Run device timers up to `now`.
    pub fn advance(&mut self, now: u64) {
        if now > self.now {
            self.now = now;
        }
        if let Some(t) = self.next_poll {
            if t <= now {
                let mut next: Option<u64> = None;
                for ci in 0..self.comps.len() {
                    if let Some(nt) = self.comps[ci].dev.poll(now) {
                        next = Some(next.map_or(nt, |x| x.min(nt)));
                    }
                    if self.comps[ci].dev.take_dirty() {
                        for n in self.comps[ci].nets.clone() {
                            self.mark_net(n);
                        }
                    }
                }
                self.next_poll = next;
            }
        }
        if !self.cap_dyn.is_empty() {
            self.cap_next = self.advance_caps(now);
        }
        if self.any_dirty {
            self.solve_dirty();
        }
    }

    pub fn log(&mut self, source: &str, text: String) {
        if self.events.len() > 500 {
            self.events.pop_front();
        }
        self.events.push_back(BoardEvent { time_ns: self.now, source: source.to_string(), text });
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.comps.iter().position(|c| c.id.eq_ignore_ascii_case(id))
    }

    /// Apply a user interaction to a component.
    pub fn input(&mut self, id: &str, action: &Value) -> Result<(), String> {
        let ci = self.find(id).ok_or_else(|| format!("no component '{}'", id))?;
        let now = self.now;
        self.comps[ci].dev.input(now, action)?;
        // an interaction may change stamps or schedule events
        for n in self.comps[ci].nets.clone() {
            self.mark_net(n);
        }
        self.next_poll = Some(now);
        self.solve_dirty();
        Ok(())
    }

    /// Snapshot of all component states (for the GUI).
    pub fn snapshot(&self) -> Value {
        let comps: Vec<Value> = self
            .comps
            .iter()
            .map(|c| json!({"id": c.id, "kind": c.dev.kind(), "state": c.dev.state(), "summary": c.dev.summary()}))
            .collect();
        let mut pads = serde_json::Map::new();
        for g in 0..NUM_GPIO {
            if self.pad_net[g].is_some() {
                let bit = 1u64 << g;
                let mode = if self.pwm[g].is_some() {
                    "pwm"
                } else if self.drive.out_en & bit != 0 {
                    "out"
                } else {
                    "in"
                };
                pads.insert(
                    g.to_string(),
                    json!({"v": self.pad_volts(g as u8), "mode": mode, "pwm": self.pwm[g].map(|p| json!({"freq": p.freq_hz, "duty": p.duty}))}),
                );
            }
        }
        json!({"time_ns": self.now, "components": comps, "pads": pads})
    }

    fn devices_on_net(&self, net: usize) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        for (ci, c) in self.comps.iter().enumerate() {
            for (pi, &n) in c.nets.iter().enumerate() {
                if n == net {
                    v.push((ci, pi));
                }
            }
        }
        v
    }

    fn uart_tx_pad(&self, port: u8) -> Option<u8> {
        let sig = match port {
            0 => 14,
            1 => 17,
            _ => 198,
        };
        (0..NUM_GPIO).find(|&g| self.drive.out_signal[g] == sig).map(|g| g as u8).or(match port {
            0 => Some(1),
            _ => None,
        })
    }
    fn uart_rx_pad(&self, port: u8) -> Option<u8> {
        self.drive.uart_rx_pads.get(port as usize).copied().flatten().or(match port {
            0 => Some(3),
            _ => None,
        })
    }

    /// I2C devices on the bus formed by these pads (or all, if the routing is unknown).
    fn i2c_targets(&self, scl: Option<u8>, sda: Option<u8>) -> Vec<usize> {
        let sn = scl.and_then(|p| self.pad_net[p as usize]);
        let dn = sda.and_then(|p| self.pad_net[p as usize]);
        self.comps
            .iter()
            .enumerate()
            .filter(|(_, c)| match c.dev.i2c_pins() {
                Some((a, b)) => match (sn, dn) {
                    (Some(s), Some(d)) => c.nets[a] == s && c.nets[b] == d,
                    _ => true,
                },
                None => false,
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Transaction-level I2C access used by the software (bit-banged) decoder.
    pub(crate) fn i2c_bus_start(&mut self, scl_net: usize, sda_net: usize, addr: u8, read: bool) -> Option<usize> {
        for ci in 0..self.comps.len() {
            if let Some((a, b)) = self.comps[ci].dev.i2c_pins() {
                if self.comps[ci].nets[a] == scl_net && self.comps[ci].nets[b] == sda_net && self.comps[ci].dev.i2c_start(addr, read) {
                    return Some(ci);
                }
            }
        }
        None
    }
    pub(crate) fn comp_mut(&mut self, ci: usize) -> &mut Box<dyn Device> {
        &mut self.comps[ci].dev
    }
}

/// Gaussian elimination with partial pivoting.
/// Transaction-level log of the buses (I2C, SPI, UART, RMT) for the bus monitor.
#[derive(Default)]
pub struct BusLog {
    pub events: VecDeque<Value>,
    pub total: u64,
    i2c: Option<Value>,
    uart: [(Vec<u8>, u64, bool); 6],
    last_spi: Option<(u64, String)>,
}

impl BusLog {
    const CAP: usize = 20_000;
    fn push(&mut self, v: Value) {
        if self.events.len() >= Self::CAP {
            self.events.pop_front();
        }
        self.events.push_back(v);
        self.total += 1;
    }
    /// Events with sequence number >= since; returns (events, next).
    pub fn since(&self, since: u64, max: usize) -> (Vec<Value>, u64) {
        let first = self.total - self.events.len() as u64;
        let start = since.max(first);
        let v: Vec<Value> = self.events.iter().skip((start - first) as usize).take(max).cloned().collect();
        let next = start + v.len() as u64;
        (v, next)
    }
    fn i2c_flush(&mut self) {
        if let Some(t) = self.i2c.take() {
            self.push(t);
        }
    }
    /// Buffer UART bytes into lines (slot = port*2 + rx).
    fn uart(&mut self, now: u64, port: u8, rx: bool, b: u8) {
        let slot = port as usize * 2 + rx as usize;
        let stale = !self.uart[slot].0.is_empty() && now.saturating_sub(self.uart[slot].1) > 5_000_000;
        if stale {
            self.uart_flush(slot);
        }
        if self.uart[slot].0.is_empty() {
            self.uart[slot].1 = now;
        }
        self.uart[slot].0.push(b);
        self.uart[slot].2 = rx;
        if b == b'\n' || self.uart[slot].0.len() >= 64 {
            self.uart_flush(slot);
        }
    }
    fn uart_flush(&mut self, slot: usize) {
        let (data, t, rx) = std::mem::take(&mut self.uart[slot]);
        if data.is_empty() {
            return;
        }
        let text: String = data.iter().filter(|&&c| c != b'\r').map(|&c| if (32..127).contains(&c) { c as char } else if c == b'\n' { '⏎' } else { '·' }).collect();
        self.push(json!({"t": t, "bus": format!("UART{}", slot / 2), "dir": if rx { "rx" } else { "tx" }, "text": text, "data": crate::parts::hex(&data)}));
    }
}

/// Ring buffer of pad level changes (logic analyzer).
pub struct PadTrace {
    pub enabled: bool,
    /// (time ns, pad, level; None = floating)
    pub events: VecDeque<(u64, u8, Option<bool>)>,
    last: [Option<bool>; NUM_GPIO],
    /// total events ever recorded (index of events[0] = total - len)
    pub total: u64,
}

impl Default for PadTrace {
    fn default() -> Self {
        PadTrace { enabled: true, events: VecDeque::new(), last: [None; NUM_GPIO], total: 0 }
    }
}

impl PadTrace {
    const CAP: usize = 400_000;
    fn push(&mut self, t: u64, pad: u8, level: Option<bool>) {
        if self.events.len() >= Self::CAP {
            self.events.pop_front();
        }
        self.events.push_back((t, pad, level));
        self.total += 1;
    }
    /// Events with sequence number >= `since` (at most `max`), and the next sequence number.
    pub fn since(&self, since: u64, max: usize) -> (Vec<(u64, u8, Option<bool>)>, u64) {
        let first = self.total - self.events.len() as u64;
        let start = since.max(first);
        let skip = (start - first) as usize;
        let v: Vec<_> = self.events.iter().skip(skip).take(max).copied().collect();
        let next = start + v.len() as u64;
        (v, next)
    }
    /// Current level of every pad (for initialising a view).
    pub fn levels(&self) -> Vec<Option<bool>> {
        self.last.to_vec()
    }
}

/// Series resistance of a capacitor while it is treated as a voltage source.
const CAP_ESR: f64 = 0.05;

/// Net-level circuit element used by the solver.
#[derive(Clone)]
enum El {
    /// resistor between two nets
    R(usize, usize, f64),
    /// source to ground: net, volts, internal resistance
    Src(usize, f64, f64),
    /// diode: anode, cathode, Vf, Ron, component, tag
    D(usize, usize, f64, f64, usize, u32),
    /// floating source a(+) b(-): conductance, volts (capacitors)
    F(usize, usize, f64, f64),
}

/// Charging state of one capacitor.
#[derive(Clone, Copy, Debug)]
struct CapDyn {
    v0: f64,
    t0: u64,
    v_th: f64,
    tau: f64,
    group: usize,
    /// voltage used in the last solve
    v_solved: f64,
}

impl CapDyn {
    fn value_at(&self, now: u64) -> f64 {
        if !self.tau.is_finite() || self.tau <= 0.0 {
            return self.v0;
        }
        let dt = now.saturating_sub(self.t0) as f64 * 1e-9;
        self.v_th + (self.v0 - self.v_th) * (-dt / self.tau).exp()
    }
}

fn rail_v(net: usize, rails: (usize, usize, usize)) -> Option<f64> {
    if net == rails.0 {
        Some(0.0)
    } else if net == rails.1 {
        Some(VDD)
    } else if net == rails.2 {
        Some(5.0)
    } else {
        None
    }
}

fn net_v(net: usize, sol: &[f64], local: &dyn Fn(usize) -> Option<usize>, rails: (usize, usize, usize)) -> f64 {
    match local(net) {
        Some(x) => sol[x],
        None => rail_v(net, rails).unwrap_or(0.0),
    }
}

/// Solve the linear network, iterating diode on/off states until consistent.
/// Returns node voltages (local indices) and the final diode states.
fn solve_nonlinear(els: &[El], n: usize, local: &dyn Fn(usize) -> Option<usize>, rails: (usize, usize, usize)) -> (Vec<f64>, Vec<bool>) {
    let nd = els.iter().filter(|e| matches!(e, El::D(..))).count();
    let mut don = vec![false; nd];
    let mut sol = vec![0.0f64; n];
    for _iter in 0..12 {
        let mut gm = vec![vec![0.0f64; n]; n];
        let mut iv = vec![0.0f64; n];
        for i in 0..n {
            gm[i][i] += G_LEAK;
        }
        let stamp_r = |gm: &mut Vec<Vec<f64>>, iv: &mut Vec<f64>, a: usize, b: usize, g: f64| match (local(a), local(b)) {
            (Some(x), Some(y)) => {
                gm[x][x] += g;
                gm[y][y] += g;
                gm[x][y] -= g;
                gm[y][x] -= g;
            }
            (Some(x), None) => {
                gm[x][x] += g;
                iv[x] += g * rail_v(b, rails).unwrap_or(0.0);
            }
            (None, Some(y)) => {
                gm[y][y] += g;
                iv[y] += g * rail_v(a, rails).unwrap_or(0.0);
            }
            _ => {}
        };
        let mut di = 0;
        for e in els {
            match *e {
                El::R(a, b, r) => stamp_r(&mut gm, &mut iv, a, b, 1.0 / r),
                El::Src(a, v, r) => {
                    if let Some(x) = local(a) {
                        gm[x][x] += 1.0 / r;
                        iv[x] += v / r;
                    }
                }
                El::F(a, b, g, v) => {
                    stamp_r(&mut gm, &mut iv, a, b, g);
                    if let Some(x) = local(a) {
                        iv[x] += g * v;
                    }
                    if let Some(y) = local(b) {
                        iv[y] -= g * v;
                    }
                }
                El::D(a, k, vf, ron, _, _) => {
                    if don[di] {
                        let gg = 1.0 / ron;
                        stamp_r(&mut gm, &mut iv, a, k, gg);
                        if let Some(x) = local(a) {
                            iv[x] += gg * vf;
                        }
                        if let Some(y) = local(k) {
                            iv[y] -= gg * vf;
                        }
                    } else {
                        stamp_r(&mut gm, &mut iv, a, k, 1e-9);
                    }
                    di += 1;
                }
            }
        }
        sol = gauss(gm, iv);
        let mut changed = false;
        let mut di = 0;
        for e in els {
            if let El::D(a, k, vf, _, _, _) = *e {
                let vak = net_v(a, &sol, local, rails) - net_v(k, &sol, local, rails);
                let on = vak > vf - 1e-6;
                let on = if don[di] { on || vak > vf - 0.01 } else { on };
                if on != don[di] {
                    don[di] = on;
                    changed = true;
                }
                di += 1;
            }
        }
        if !changed {
            break;
        }
    }
    (sol, don)
}

fn gauss(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-30 {
            continue;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..n {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for c in r + 1..n {
            s -= a[r][c] * x[c];
        }
        x[r] = if a[r][r].abs() < 1e-30 { 0.0 } else { s / a[r][r] };
    }
    x
}

struct SelectedI2c {
    comps: Vec<usize>,
}

thread_local! {
    static I2C_SEL: std::cell::RefCell<Option<SelectedI2c>> = const { std::cell::RefCell::new(None) };
}

impl IoBridge for Board {
    fn gpio_update(&mut self, now: u64, drive: &PinDrive) {
        self.advance(now);
        let old = std::mem::replace(&mut self.drive, drive.clone());
        for g in 0..NUM_GPIO {
            if let Some(net) = self.pad_net[g] {
                let bit = 1u64 << g;
                let changed = ((old.out_en ^ drive.out_en)
                    | (old.out_level ^ drive.out_level)
                    | (old.open_drain ^ drive.open_drain)
                    | (old.pull_up ^ drive.pull_up)
                    | (old.pull_down ^ drive.pull_down))
                    & bit
                    != 0
                    || (g >= 25 && g <= 26 && old.dac[g - 25] != drive.dac[g - 25]);
                if changed {
                    self.mark_net(net);
                }
            }
        }
        self.solve_dirty();
    }

    fn gpio_read(&mut self, now: u64, pin: u8) -> Option<bool> {
        self.advance(now);
        self.pad_level(pin)
    }

    fn adc_read(&mut self, now: u64, pin: u8) -> f32 {
        self.advance(now);
        self.pad_volts(pin).unwrap_or(0.0) as f32
    }

    fn uart_tx(&mut self, now: u64, port: u8, byte: u8) {
        self.advance(now);
        self.uart_counts[port as usize % 3] += 1;
        self.buslog.uart(now, port, false, byte);
        if port == 0 {
            self.console_out.push_back(byte);
            if self.console_out.len() > 1 << 20 {
                self.console_out.pop_front();
            }
        }
        if let Some(pad) = self.uart_tx_pad(port) {
            if let Some(net) = self.pad_net[pad as usize] {
                for (ci, pi) in self.devices_on_net(net) {
                    if let Some((rx, _tx)) = self.comps[ci].dev.uart_pins() {
                        if rx == pi {
                            self.comps[ci].dev.uart_receive(now, byte);
                        }
                    }
                }
            }
        }
    }

    fn uart_rx(&mut self, now: u64, port: u8) -> Option<u8> {
        self.advance(now);
        if port == 0 {
            if let Some(b) = self.console_in.pop_front() {
                self.buslog.uart(now, port, true, b);
                return Some(b);
            }
        }
        let pad = self.uart_rx_pad(port)?;
        let net = self.pad_net[pad as usize]?;
        for (ci, pi) in self.devices_on_net(net) {
            if let Some((_rx, tx)) = self.comps[ci].dev.uart_pins() {
                if tx == pi {
                    if let Some(b) = self.comps[ci].dev.uart_transmit(now) {
                        self.buslog.uart(now, port, true, b);
                        return Some(b);
                    }
                }
            }
        }
        None
    }

    fn i2c_start(&mut self, now: u64, scl: Option<u8>, sda: Option<u8>, addr7: u8, read: bool) -> bool {
        self.advance(now);
        let targets = self.i2c_targets(scl, sda);
        let mut acked = Vec::new();
        for ci in targets {
            // bring the device's clock up to date first: devices without timed
            // behaviour are not polled otherwise (e.g. an RTC chip read over I2C)
            if let Some(nt) = self.comps[ci].dev.poll(now) {
                self.next_poll = Some(self.next_poll.map_or(nt, |x| x.min(nt)));
            }
            if self.comps[ci].dev.i2c_start(addr7, read) {
                acked.push(ci);
            }
        }
        let ok = !acked.is_empty();
        self.buslog.i2c_flush();
        let devs: Vec<String> = acked.iter().map(|&ci| self.comps[ci].id.clone()).collect();
        self.buslog.i2c = Some(json!({"t": now, "bus": "I2C", "dir": if read { "read" } else { "write" },
            "addr": format!("0x{:02X}", addr7), "ack": ok, "devices": devs, "data": "", "pads": [scl, sda]}));
        I2C_SEL.with(|s| *s.borrow_mut() = Some(SelectedI2c { comps: acked }));
        ok
    }

    fn i2c_write(&mut self, now: u64, _scl: Option<u8>, _sda: Option<u8>, byte: u8) -> bool {
        self.advance(now);
        let sel = I2C_SEL.with(|s| s.borrow().as_ref().map(|x| x.comps.clone())).unwrap_or_default();
        let mut ack = false;
        for ci in sel {
            ack |= self.comps[ci].dev.i2c_write(byte);
            if self.comps[ci].dev.take_dirty() {
                for n in self.comps[ci].nets.clone() {
                    self.mark_net(n);
                }
            }
        }
        if let Some(t) = self.buslog.i2c.as_mut() {
            let d = t["data"].as_str().unwrap_or("").to_string();
            if d.len() < 128 {
                t["data"] = json!(format!("{}{:02x}{}", d, byte, if ack { "" } else { "!" }));
            }
        }
        ack
    }

    fn i2c_read(&mut self, now: u64, _scl: Option<u8>, _sda: Option<u8>, ack: bool) -> u8 {
        self.advance(now);
        let sel = I2C_SEL.with(|s| s.borrow().as_ref().map(|x| x.comps.clone())).unwrap_or_default();
        let mut v = 0xFFu8;
        for ci in sel {
            v &= self.comps[ci].dev.i2c_read(ack);
        }
        if let Some(t) = self.buslog.i2c.as_mut() {
            let d = t["data"].as_str().unwrap_or("").to_string();
            if d.len() < 128 {
                t["data"] = json!(format!("{}{:02x}", d, v));
            }
        }
        v
    }

    fn i2c_stop(&mut self, now: u64, _scl: Option<u8>, _sda: Option<u8>) {
        self.advance(now);
        let sel = I2C_SEL.with(|s| s.borrow_mut().take().map(|x| x.comps)).unwrap_or_default();
        self.buslog.i2c_flush();
        for ci in sel {
            self.comps[ci].dev.i2c_stop();
            if self.comps[ci].dev.take_dirty() {
                for n in self.comps[ci].nets.clone() {
                    self.mark_net(n);
                }
            }
        }
        self.solve_dirty();
    }

    fn spi_transfer(&mut self, now: u64, _host: u8, pins: SpiPins, data: &mut [u8], _bits: u32) {
        self.advance(now);
        let mosi_copy = data.to_vec();
        let mut selected_ids: Vec<String> = Vec::new();
        let sck = pins.sclk.and_then(|p| self.pad_net[p as usize]);
        let mosi = pins.mosi.and_then(|p| self.pad_net[p as usize]);
        let hw_cs = pins.cs.and_then(|p| self.pad_net[p as usize]);
        let mut done = false;
        for ci in 0..self.comps.len() {
            if let Some((s, m, _mi, cs)) = self.comps[ci].dev.spi_pins() {
                let c = &self.comps[ci];
                let on_bus = sck.map(|n| c.nets[s] == n).unwrap_or(false) || mosi.map(|n| c.nets[m] == n).unwrap_or(false);
                if !on_bus {
                    continue;
                }
                let cs_net = c.nets[cs];
                let selected = Some(cs_net) == hw_cs || self.net_level(cs_net) == Some(false);
                if !selected {
                    continue;
                }
                selected_ids.push(c.id.clone());
                let nets = c.nets.clone();
                let levels: Vec<bool> = nets.iter().map(|&n| self.net_level(n).unwrap_or(false)).collect();
                let lv = move |p: usize| levels.get(p).copied().unwrap_or(false);
                let mut buf = data.to_vec();
                let hw = Some(cs_net) == hw_cs;
                if hw {
                    // the controller toggles CS itself: one transfer is one transaction
                    self.comps[ci].dev.spi_select(true);
                }
                self.comps[ci].dev.spi_transfer(now, &mut buf, &lv);
                if hw {
                    self.comps[ci].dev.spi_select(false);
                }
                if !done {
                    data.copy_from_slice(&buf);
                    done = true;
                }
                if self.comps[ci].dev.take_dirty() {
                    for n in nets {
                        self.mark_net(n);
                    }
                }
            }
        }
        let mosi_hex = crate::parts::hex(&mosi_copy[..mosi_copy.len().min(48)]);
        if !done {
            for b in data.iter_mut() {
                *b = 0xFF;
            }
        }
        let miso_hex = crate::parts::hex(&data[..data.len().min(48)]);
        let devs: Vec<String> = selected_ids;
        let key = format!("{:?}{:?}", pins, devs);
        // consecutive small transfers to the same device within 100 us form one entry
        let merge = matches!(&self.buslog.last_spi, Some((t, k)) if *k == key && now.saturating_sub(*t) < 100_000) && self.buslog.events.back().map(|e| e["bus"] == "SPI").unwrap_or(false);
        if merge {
            if let Some(e) = self.buslog.events.back_mut() {
                let (m, i) = (e["mosi"].as_str().unwrap_or("").to_string(), e["miso"].as_str().unwrap_or("").to_string());
                if m.len() < 96 {
                    e["mosi"] = json!(m + &mosi_hex);
                    e["miso"] = json!(i + &miso_hex);
                }
                e["len"] = json!(e["len"].as_u64().unwrap_or(0) + data.len() as u64);
            }
        } else {
            self.buslog.push(json!({"t": now, "bus": "SPI", "dir": "xfer", "devices": devs, "len": data.len(), "mosi": mosi_hex, "miso": miso_hex,
                "pads": [pins.sclk, pins.mosi, pins.miso, pins.cs]}));
        }
        self.buslog.last_spi = Some((now, key));
    }

    fn rmt_tx(&mut self, now: u64, pin: u8, items: &[RmtItem]) {
        self.advance(now);
        let dur: u64 = items.iter().map(|i| i.duration_ns as u64).sum();
        self.buslog.push(json!({"t": now, "bus": "RMT", "dir": "tx", "pads": [pin], "len": items.len(), "text": format!("{} pulses, {:.1} µs on GPIO{}", items.len(), dur as f64 / 1000.0, pin)}));
        if let Some(net) = self.pad_net[pin as usize] {
            for (ci, pi) in self.devices_on_net(net) {
                self.comps[ci].dev.waveform(now, pi, items);
            }
        }
    }

    fn pwm_update(&mut self, now: u64, pin: u8, pwm: PwmState) {
        self.advance(now);
        let st = if pwm.running { Some(pwm) } else { None };
        self.pwm[pin as usize] = st;
        if let Some(net) = self.pad_net[pin as usize] {
            for (ci, pi) in self.devices_on_net(net) {
                self.comps[ci].dev.pwm(now, pi, pwm);
            }
            self.mark_net(net);
            self.solve_dirty();
        }
    }

    fn poll(&mut self, now: u64) -> Option<u64> {
        self.advance(now);
        let mut next = match (self.next_poll, self.cap_next) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if !self.console_in.is_empty() {
            next = Some(now);
        }
        for ci in 0..self.comps.len() {
            if self.comps[ci].dev.uart_pins().is_some() {
                // devices with pending UART output need the receiver polled
                next = Some(next.map_or(now + 100_000, |n| n.min(now + 100_000)));
                break;
            }
        }
        next
    }

    fn touch_read(&mut self, now: u64, pin: u8) -> bool {
        self.advance(now);
        let Some(net) = self.pad_net.get(pin as usize).copied().flatten() else { return false };
        self.devices_on_net(net).into_iter().any(|(ci, pi)| self.comps[ci].dev.touched(pi))
    }

    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}
