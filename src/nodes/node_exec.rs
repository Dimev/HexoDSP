// Copyright (c) 2021 Weird Constructor <weirdconstructor@gmail.com>
// This file is a part of HexoDSP. Released under GPL-3.0-or-later.
// See README.md and COPYING for details.

use super::{
    DropMsg, GraphMessage, NodeProg, FB_DELAY_TIME_US, MAX_ALLOCATED_NODES, MAX_FB_DELAY_SIZE,
    MAX_SMOOTHERS, UNUSED_MONITOR_IDX,
};
use crate::dsp::{Node, NodeContext, NodeId, MAX_BLOCK_SIZE};
use crate::monitor::{MonitorBackend, MON_SIG_CNT};
use crate::util::Smoother;
use synfx_dsp::AtomicFloat;

use crate::log;
use std::io::Write;

use ringbuf::{Consumer, Producer};
use std::sync::Arc;

use core::arch::x86_64::{
    _MM_FLUSH_ZERO_ON,
    //    _MM_FLUSH_ZERO_OFF,
    _MM_SET_FLUSH_ZERO_MODE,
    //    _MM_GET_FLUSH_ZERO_MODE
};

/// Holds the complete allocation of nodes and
/// the program. New Nodes or the program is
/// not newly allocated in the audio backend, but it is
/// copied from the input ring buffer.
/// If this turns out to be too slow, we might
/// have to push buffers of the program around.
///
pub struct NodeExecutor {
    /// Contains the nodes and their state.
    /// Is loaded from the input ring buffer when a corresponding
    /// message arrives.
    ///
    /// In case the previous node contained something that needs
    /// deallocation, the nodes are replaced and the contents
    /// is sent back using the free-ringbuffer.
    pub(crate) nodes: Vec<Node>,

    /// Contains the stand-by smoothing operators for incoming parameter changes.
    pub(crate) smoothers: Vec<(usize, Smoother)>,

    /// Contains target parameter values after a smoother finished,
    /// these will refresh the input buffers:
    pub(crate) target_refresh: Vec<(usize, f32)>,

    /// Contains the to be executed nodes and output operations.
    /// Is copied from the input ringbuffer when a corresponding
    /// message arrives.
    pub(crate) prog: NodeProg,

    /// Holds the input vector indices which are to be monitored by the frontend.
    pub(crate) monitor_signal_cur_inp_indices: [usize; MON_SIG_CNT],

    /// The sample rate
    pub(crate) sample_rate: f32,

    /// Context that can be accessed by all (executed) nodes at runtime.
    pub(crate) exec_ctx: NodeExecContext,

    /// The connection with the [crate::nodes::NodeConfigurator].
    shared: SharedNodeExec,

    /// A flag to remember if we already initialized the logger on the audio thread.
    dsp_log_init: bool,
}

/// Contains anything that connects the [NodeExecutor] with the frontend part.
pub(crate) struct SharedNodeExec {
    /// Holds two context values interleaved.
    /// The first for each node is the LED value and the second is a
    /// phase value. The LED will be displayed in the hex matrix, while the
    /// phase might be used to display an envelope's play position.
    pub(crate) node_ctx_values: Vec<Arc<AtomicFloat>>,
    /// For receiving Node and NodeProg updates
    pub(crate) graph_update_con: Consumer<GraphMessage>,
    /// For receiving deleted/overwritten nodes from the backend thread.
    pub(crate) graph_drop_prod: Producer<DropMsg>,
    /// For sending feedback to the frontend thread.
    pub(crate) monitor_backend: MonitorBackend,
}

/// Contains audio driver context informations. Such as the number
/// of frames of the current buffer period and allows
/// writing output samples and reading input samples.
pub trait NodeAudioContext {
    fn nframes(&self) -> usize;
    fn output(&mut self, channel: usize, frame: usize, v: f32);
    fn input(&mut self, channel: usize, frame: usize) -> f32;
}

/// Implements a trivial buffer for the feedback nodes
/// FbWr and FbRd.
///
/// Note that the previous audio period or even the first one ever may
/// produce less than 128 samples. This means we need to keep track
/// how many samples were actually written to the feedback buffer!
/// See also `sample_count` field.
pub struct FeedbackBuffer {
    /// The feedback buffer that holds the samples of the previous period.
    buffer: [f32; MAX_FB_DELAY_SIZE],
    /// The write pointer.
    write_ptr: usize,
    /// Read pointer, is always behind write_ptr by an initial amount
    read_ptr: usize,
}

impl FeedbackBuffer {
    pub fn new() -> Self {
        let delay_sample_count = (44100.0 as usize * FB_DELAY_TIME_US) / 1000000;
        Self {
            buffer: [0.0; MAX_FB_DELAY_SIZE],
            write_ptr: delay_sample_count % MAX_FB_DELAY_SIZE,
            read_ptr: 0,
        }
    }

    pub fn clear(&mut self) {
        self.buffer = [0.0; MAX_FB_DELAY_SIZE];
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.buffer = [0.0; MAX_FB_DELAY_SIZE];
        // The delay sample count maximum is defined by MAX_FB_DELAY_SRATE,
        // after that the feedback delays become shorter than they should be
        // and things won't sound the same at sample rate
        // exceeding MAX_FB_DELAY_SRATE.
        //
        // This is a tradeoff of wasted memory and not having to reallocate
        // these delays for sample rate changes and providing delay buffers
        // for all 256 theoretical feedback delay nodes.
        //
        // For more elaborate and longer delays an extra delay node should
        // be used before FbWr or after FbRd.

        let delay_sample_count = (sr as usize * FB_DELAY_TIME_US) / 1000000;
        self.write_ptr = delay_sample_count % MAX_FB_DELAY_SIZE;
        self.read_ptr = 0;
    }

    #[inline]
    pub fn write(&mut self, s: f32) {
        self.write_ptr = (self.write_ptr + 1) % MAX_FB_DELAY_SIZE;
        self.buffer[self.write_ptr] = s;
        //d// println!("WRITE[{}] = {:8.3}", self.write_ptr, s);
    }

    #[inline]
    pub fn read(&mut self) -> f32 {
        self.read_ptr = (self.read_ptr + 1) % MAX_FB_DELAY_SIZE;
        self.buffer[self.read_ptr]
        //d// let s = self.buffer[self.read_ptr];
        //d// println!("READ[{}] = {:8.3}", self.read_ptr, s);
        //d// s
    }
}

impl Default for FeedbackBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Contains global state that all nodes can access.
/// This is used for instance to implement the feedbackd delay nodes.
pub struct NodeExecContext {
    pub feedback_delay_buffers: Vec<FeedbackBuffer>,
}

impl NodeExecContext {
    fn new() -> Self {
        let mut fbdb = vec![];
        fbdb.resize_with(MAX_ALLOCATED_NODES, FeedbackBuffer::new);
        Self { feedback_delay_buffers: fbdb }
    }

    fn set_sample_rate(&mut self, srate: f32) {
        for b in self.feedback_delay_buffers.iter_mut() {
            b.set_sample_rate(srate);
        }
    }

    fn clear(&mut self) {
        for b in self.feedback_delay_buffers.iter_mut() {
            b.clear();
        }
    }
}

impl NodeExecutor {
    pub(crate) fn new(shared: SharedNodeExec) -> Self {
        let mut nodes = Vec::new();
        nodes.resize_with(MAX_ALLOCATED_NODES, || Node::Nop);

        let mut smoothers = Vec::new();
        smoothers.resize_with(MAX_SMOOTHERS, || (0, Smoother::new()));

        let target_refresh = Vec::with_capacity(MAX_SMOOTHERS);

        NodeExecutor {
            nodes,
            smoothers,
            target_refresh,
            sample_rate: 44100.0,
            prog: NodeProg::empty(),
            monitor_signal_cur_inp_indices: [UNUSED_MONITOR_IDX; MON_SIG_CNT],
            exec_ctx: NodeExecContext::new(),
            dsp_log_init: false,
            shared,
        }
    }

    pub fn no_logging(&mut self) {
        self.dsp_log_init = true;
    }

    #[inline]
    pub fn process_graph_updates(&mut self) {
        while let Some(upd) = self.shared.graph_update_con.pop() {
            match upd {
                GraphMessage::NewNode { index, mut node } => {
                    node.set_sample_rate(self.sample_rate);
                    let prev_node = std::mem::replace(&mut self.nodes[index as usize], node);

                    log(|w| {
                        let _ = write!(w, "[dbg] Create node index={}", index);
                    });

                    let _ = self.shared.graph_drop_prod.push(DropMsg::Node { node: prev_node });
                }
                GraphMessage::Clear { prog } => {
                    for n in self.nodes.iter_mut() {
                        if n.to_id(0) != NodeId::Nop {
                            let prev_node = std::mem::replace(n, Node::Nop);
                            let _ =
                                self.shared.graph_drop_prod.push(DropMsg::Node { node: prev_node });
                        }
                    }

                    self.exec_ctx.clear();

                    self.monitor_signal_cur_inp_indices = [UNUSED_MONITOR_IDX; MON_SIG_CNT];

                    log(|w| {
                        let _ = write!(w, "[dbg] Cleared graph ({} nodes)", self.prog.prog.len());
                    });

                    let prev_prog = std::mem::replace(&mut self.prog, prog);
                    let _ = self.shared.graph_drop_prod.push(DropMsg::Prog { prog: prev_prog });
                }
                GraphMessage::NewProg { prog, copy_old_out } => {
                    let mut prev_prog = std::mem::replace(&mut self.prog, prog);

                    unsafe {
                        _MM_SET_FLUSH_ZERO_MODE(_MM_FLUSH_ZERO_ON);
                    }

                    self.monitor_signal_cur_inp_indices = [UNUSED_MONITOR_IDX; MON_SIG_CNT];

                    // XXX: Copying from the old vector works, because we only
                    //      append nodes to the _end_ of the node instance vector.
                    //      If we do a garbage collection, we can't do this.
                    //
                    // XXX: Also, we need to initialize the input parameter
                    //      vector, because we don't know if they are updated from
                    //      the new program outputs anymore. So we need to
                    //      copy the old paramters to the inputs.
                    //
                    //      => This does not apply to atom data, because that
                    //         is always sent with the new program and "should"
                    //         be up to date, even if we have a slight possible race
                    //         condition between GraphMessage::NewProg
                    //         and GraphMessage::AtomUpdate.

                    // First overwrite by the current input parameters,
                    // to make sure _all_ inputs have a proper value
                    // (not just those that existed before).
                    //
                    // We preserve the modulation history in the next step.
                    // This is also to make sure that new input ports
                    // have a proper value too.
                    self.prog.initialize_input_buffers();

                    if copy_old_out {
                        // XXX: The following is commented out, because presisting
                        //      the output proc buffers does not make sense anymore.
                        //      Because we don't allow cycles, so there is no
                        //      way that a node can read from the previous
                        //      iteration anyways.
                        //
                        // // Swap the old out buffers into the new NodeProg
                        // // TODO: If we toss away most of the buffers anyways,
                        // //       we could optimize this step with more
                        // //       intelligence in the matrix compiler.
                        // for (old_pb, new_pb) in
                        //     prev_prog.out.iter_mut().zip(
                        //         self.prog.out.iter_mut())
                        // {
                        //     std::mem::swap(old_pb, new_pb);
                        // }

                        // Then overwrite the inputs by the more current previous
                        // input processing buffers, so we keep any modulation
                        // (smoothed) history of the block too.
                        self.prog.swap_previous_outputs(&mut prev_prog);
                    }

                    self.prog.assign_outputs();

                    let _ = self.shared.graph_drop_prod.push(DropMsg::Prog { prog: prev_prog });

                    log(|w| {
                        let _ = write!(
                            w,
                            "[dbg] Created new graph (node count={})",
                            self.prog.prog.len()
                        );
                    });
                }
                GraphMessage::AtomUpdate { at_idx, value } => {
                    let prog = &mut self.prog;
                    let garbage = std::mem::replace(&mut prog.atoms[at_idx], value);

                    let _ = self.shared.graph_drop_prod.push(DropMsg::Atom { atom: garbage });
                }
                GraphMessage::ParamUpdate { input_idx, value } => {
                    self.set_param(input_idx, value);
                }
                GraphMessage::ModamtUpdate { mod_idx, modamt } => {
                    self.set_modamt(mod_idx, modamt);
                }
                GraphMessage::SetMonitor { bufs } => {
                    self.monitor_signal_cur_inp_indices = bufs;
                }
            }
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.exec_ctx.set_sample_rate(sample_rate);
        for n in self.nodes.iter_mut() {
            n.set_sample_rate(sample_rate);
        }

        for sm in self.smoothers.iter_mut() {
            sm.1.set_sample_rate(sample_rate);
        }
    }

    #[inline]
    pub fn get_nodes(&self) -> &Vec<Node> {
        &self.nodes
    }

    #[inline]
    pub fn get_prog(&self) -> &NodeProg {
        &self.prog
    }

    #[inline]
    fn set_modamt(&mut self, mod_idx: usize, modamt: f32) {
        if mod_idx < self.prog.modops.len() {
            self.prog.modops[mod_idx].set_amt(modamt);
        }
    }

    #[inline]
    fn set_param(&mut self, input_idx: usize, value: f32) {
        let prog = &mut self.prog;

        if input_idx >= prog.params.len() {
            return;
        }

        // First check if we already have a running smoother for this param:
        for (sm_inp_idx, smoother) in self.smoothers.iter_mut().filter(|s| !s.1.is_done()) {
            if *sm_inp_idx == input_idx {
                smoother.set(prog.params[input_idx], value);
                //d// println!("RE-SET SMOOTHER {} {:6.3} (old = {:6.3})",
                //d//          input_idx, value, prog.params[input_idx]);
                return;
            }
        }

        // Find unused smoother and set it:
        if let Some(sm) = self.smoothers.iter_mut().find(|s| s.1.is_done()) {
            sm.0 = input_idx;
            sm.1.set(prog.params[input_idx], value);
            //d// println!("SET SMOOTHER {} {:6.3} (old = {:6.3})",
            //d//          input_idx, value, prog.params[input_idx]);
        }
    }

    #[inline]
    fn process_smoothers(&mut self, nframes: usize) {
        let prog = &mut self.prog;

        while let Some((idx, v)) = self.target_refresh.pop() {
            prog.inp[idx].fill(v);
        }

        for (idx, smoother) in self.smoothers.iter_mut().filter(|s| !s.1.is_done()) {
            let inp = &mut prog.inp[*idx];
            let mut last_v = 0.0;

            for frame in 0..nframes {
                let v = smoother.next();

                inp.write(frame, v);
                last_v = v;
            }

            prog.params[*idx] = last_v;
            self.target_refresh.push((*idx, last_v));
        }
    }

    #[inline]
    pub fn process<T: NodeAudioContext>(&mut self, ctx: &mut T) {
        // let tb = std::time::Instant::now();

        if !self.dsp_log_init && crate::log::init_thread_logger("dsp") {
            self.dsp_log_init = true;
            crate::log(|w| {
                let _ = write!(w, "DSP thread logger initialized");
            });
        }

        self.process_smoothers(ctx.nframes());

        let nodes = &mut self.nodes;
        let ctx_vals = &mut self.shared.node_ctx_values;
        let prog = &mut self.prog;
        let exec_ctx = &mut self.exec_ctx;

        let prog_out_fb = prog.out_feedback.input_buffer();

        let nframes = ctx.nframes();

        for op in prog.prog.iter() {
            let out = op.out_idxlen;
            let inp = op.in_idxlen;
            let at = op.at_idxlen;
            let md = op.mod_idxlen;
            let ctx_idx = op.idx as usize * 2;

            for modop in prog.modops[md.0..md.1].iter_mut() {
                modop.process(nframes);
            }

            nodes[op.idx as usize].process(
                ctx,
                exec_ctx,
                &NodeContext {
                    out_connected: op.out_connected,
                    in_connected: op.in_connected,
                    params: &prog.inp[inp.0..inp.1],
                },
                &prog.atoms[at.0..at.1],
                &prog.cur_inp[inp.0..inp.1],
                &mut prog.out[out.0..out.1],
                &ctx_vals[ctx_idx..ctx_idx + 2],
            );

            let last_frame_idx = nframes - 1;
            for (pb, out_buf_idx) in prog.out[out.0..out.1].iter().zip(out.0..out.1) {
                prog_out_fb[out_buf_idx] = pb.read(last_frame_idx);
            }
        }

        prog.out_feedback.publish();

        self.shared.monitor_backend.check_recycle();

        // let ta = std::time::Instant::now();

        for (i, idx) in self.monitor_signal_cur_inp_indices.iter().enumerate() {
            if *idx == UNUSED_MONITOR_IDX {
                continue;
            }

            if let Some(mut mon) = self.shared.monitor_backend.get_unused_mon_buf() {
                if i > 2 {
                    mon.feed(i, ctx.nframes(), &prog.out[*idx]);
                } else {
                    mon.feed(i, ctx.nframes(), &prog.cur_inp[*idx]);
                }

                self.shared.monitor_backend.send_mon_buf(mon);
            }
        }

        // let ta = std::time::Instant::now().duration_since(ta);
        // let tb = std::time::Instant::now().duration_since(tb);
        // println!("ta Elapsed: {:?}", ta);
        // println!("tb Elapsed: {:?}", tb);
    }

    /// This is a convenience function used for testing
    /// the DSP graph output in automated tests for this crate.
    ///
    /// The sample rate that is used to run the DSP code is 44100 Hz.
    ///
    /// Relying on the behvaiour of this function for production code
    /// is not it's intended usecase and changes might break your code.
    ///
    /// * `seconds`: The number of seconds to run the DSP thread for.
    /// * `realtime`: If this is set, the function will sleep.
    ///
    /// You can use it's source as reference for your own audio
    /// DSP thread processing function.
    pub fn test_run(&mut self, seconds: f32, realtime: bool) -> (Vec<f32>, Vec<f32>) {
        const SAMPLE_RATE: f32 = 44100.0;
        self.set_sample_rate(SAMPLE_RATE);
        self.process_graph_updates();

        let mut nframes = (seconds * SAMPLE_RATE) as usize;

        let input = vec![0.0; nframes];
        let mut output_l = vec![0.0; nframes];
        let mut output_r = vec![0.0; nframes];

        for i in 0..nframes {
            output_l[i] = 0.0;
            output_r[i] = 0.0;
        }
        let mut offs = 0;
        while nframes > 0 {
            let cur_nframes = if nframes >= MAX_BLOCK_SIZE { MAX_BLOCK_SIZE } else { nframes };
            nframes -= cur_nframes;

            let mut context = crate::Context {
                nframes: cur_nframes,
                output: &mut [
                    &mut output_l[offs..(offs + cur_nframes)],
                    &mut output_r[offs..(offs + cur_nframes)],
                ],
                input: &[&input[offs..(offs + cur_nframes)]],
            };

            self.process(&mut context);

            if realtime {
                let micros = ((MAX_BLOCK_SIZE as u64) * 1000000) / (SAMPLE_RATE as u64);
                std::thread::sleep(std::time::Duration::from_micros(micros));
            }

            offs += cur_nframes;
        }

        (output_l, output_r)
    }
}
