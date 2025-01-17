// Copyright (c) 2021 Weird Constructor <weirdconstructor@gmail.com>
// This file is a part of HexoDSP. Released under GPL-3.0-or-later.
// See README.md and COPYING for details.

use synfx_dsp::AllPass;
use crate::dsp::{DspNode, LedPhaseVals, NodeContext, NodeId, ProcBuf, SAtom};
use crate::nodes::{NodeAudioContext, NodeExecContext};

/// A simple amplifier
#[derive(Debug, Clone)]
pub struct AllP {
    allpass: Box<AllPass<f64>>,
}

impl AllP {
    pub fn new(_nid: &NodeId) -> Self {
        Self { allpass: Box::new(AllPass::new()) }
    }

    pub const inp: &'static str =
        "AllP inp\nThe signal input for the allpass filter.\nRange: (-1..1)";
    pub const g: &'static str =
        "AllP g\nThe internal factor for the allpass filter.\nRange: (-1..1)";
    pub const time: &'static str = "AllP time\nThe allpass delay time.\nRange: (0..1)";
    pub const sig: &'static str = "AllP sig\nThe output of allpass filter.\nRange: (-1..1)";

    pub const DESC: &'static str = r#"Single Allpass Filter

This is an allpass filter that can be used to build reverbs
or anything you might find it useful for.
"#;
    pub const HELP: &'static str = r#"AllP - A Simple Single Allpass Filter

This is an allpass filter that can be used to build reverbs
or anything you might find it useful for.

Typical arrangements are (Schroeder Reverb):

                    t=4.5ms
                    g=0.7   -> Comb
    AllP -> AllP -> AllP -> -> Comb
    t=42ms  t=13.5ms        -> Comb
    g=0.7   g=0.7           -> Comb

Or:

    Comb ->                 t=0.48ms
    Comb ->                 g=0.7
    Comb -> AllP -> AllP -> AllP
    Comb -> t=5ms   t=1.68ms
            g=0.7   g=0.7

Typical values for the comb filters are in the range g=0.6 to 0.9
and time in the range of 30ms to 250ms.

Feel free to deviate from this and experiment around.

Building your own reverbs is fun!

(And don't forget that you can create feedback
using the FbWr and FbRd nodes!)
"#;
}

impl DspNode for AllP {
    fn outputs() -> usize {
        1
    }

    fn set_sample_rate(&mut self, srate: f32) {
        self.allpass.set_sample_rate(srate as f64);
    }

    fn reset(&mut self) {
        self.allpass.reset();
    }

    #[inline]
    fn process<T: NodeAudioContext>(
        &mut self,
        ctx: &mut T,
        _ectx: &mut NodeExecContext,
        _nctx: &NodeContext,
        _atoms: &[SAtom],
        inputs: &[ProcBuf],
        outputs: &mut [ProcBuf],
        ctx_vals: LedPhaseVals,
    ) {
        use crate::dsp::{denorm, inp, out};

        let inp = inp::AllP::inp(inputs);
        let time = inp::AllP::time(inputs);
        let g = inp::AllP::g(inputs);
        let out = out::AllP::sig(outputs);

        let ap = &mut *self.allpass;

        for frame in 0..ctx.nframes() {
            let v = inp.read(frame);

            out.write(
                frame,
                ap.next(
                    denorm::AllP::time(time, frame) as f64,
                    denorm::AllP::g(g, frame) as f64,
                    v as f64,
                ) as f32,
            );
        }

        let last_frame = ctx.nframes() - 1;
        ctx_vals[0].set(out.read(last_frame));
    }
}
