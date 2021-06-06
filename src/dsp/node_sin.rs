// Copyright (c) 2021 Weird Constructor <weirdconstructor@gmail.com>
// This is a part of HexoDSP. Released under (A)GPLv3 or any later.
// See README.md and COPYING for details.

use crate::nodes::{NodeAudioContext, NodeExecContext};
use crate::dsp::{
    NodeId, SAtom, ProcBuf, denorm, denorm_offs,
    out, inp, DspNode, LedPhaseVals
};
use crate::dsp::helpers::fast_sin;


/// A sine oscillator
#[derive(Debug, Clone)]
pub struct Sin {
    /// Sample rate
    srate: f32,
    /// Oscillator phase
    phase: f32,
}

const TWOPI : f32 = 2.0 * std::f32::consts::PI;

impl Sin {
    pub fn new(_nid: &NodeId) -> Self {
        Self {
            srate: 44100.0,
            phase: 0.0,
        }
    }
    pub const freq : &'static str =
        "Sin freq\nFrequency of the oscillator.\n\nRange: (-1..1)\n";
    pub const det : &'static str =
        "Sin det\nDetune the oscillator in semitones and cents. \
         the input of this value is rounded to semitones on coarse input. \
         Fine input lets you detune in cents (rounded). \
         A signal sent to this port is not rounded.\n\
         Note: The signal input allows detune +-10 octaves.\
         \nRange: (Knob -0.2 .. 0.2) / (Signal -1.0 .. 1.0)\n";
    pub const sig : &'static str =
        "Sin sig\nOscillator signal output.\n\nRange: (-1..1)\n";
}

impl DspNode for Sin {
    fn outputs() -> usize { 1 }

    fn set_sample_rate(&mut self, srate: f32) {
        self.srate = srate;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    #[inline]
    fn process<T: NodeAudioContext>(
        &mut self, ctx: &mut T, _ectx: &mut NodeExecContext,
        _atoms: &[SAtom], _params: &[ProcBuf], inputs: &[ProcBuf],
        outputs: &mut [ProcBuf], ctx_vals: LedPhaseVals)
    {
        let o    = out::Sin::sig(outputs);
        let freq = inp::Sin::freq(inputs);
        let det  = inp::Sin::det(inputs);
        let isr  = 1.0 / self.srate;

        let mut last_val = 0.0;
        for frame in 0..ctx.nframes() {
            let freq = denorm_offs::Sampl::freq(freq, det.read(frame), frame);

            last_val = fast_sin(self.phase * TWOPI);
            o.write(frame, last_val);

            self.phase += freq * isr;
            self.phase = self.phase.fract();
        }

        ctx_vals[0].set(last_val);
    }
}
