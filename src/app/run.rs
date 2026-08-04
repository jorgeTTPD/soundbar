use anyhow::Result;
use soundbar::{
    analyzer::{FlowSpringParams, SpectrumAnalyzer},
    audio::{AudioController, AudioMode},
    buffer::SharedBuf,
    config::Config,
    dsp::hann,
    filterbank::{build_filterbank, FilterbankParams},
};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::{
    io::{stdout, BufWriter, Write},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use super::{
    fft::{compute_spectrum, FftContext},
    gate::GateState,
    mix::compute_rms,
};

#[allow(clippy::arithmetic_side_effects)]
fn ring_cap(fft_size: usize) -> usize {
    ((48_000usize / 10).max(fft_size * 3))
        .max(fft_size * 6)
        .next_power_of_two()
}

struct AppResources {
    fft_size: usize,
    audio: AudioController,
    mic_shared: Arc<Mutex<SharedBuf>>,
    sys_shared: Arc<Mutex<SharedBuf>>,
    sr: f32,
}

struct FrameState {
    analyzer: SpectrumAnalyzer,
    gate: GateState,
    window: Vec<f32>,
    half: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    buf: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
    spec_pow: Vec<f32>,
    mic_tail: Vec<f32>,
    sys_tail: Vec<f32>,
    mix: Vec<f32>,
    bars: usize,
    dt_s: f32,
}

#[derive(Clone, Copy)]
struct Args {
    bars: usize,
    levels: u8,
    frame_ms: u64,
    mode: AudioMode,
}

fn print_usage(prog: &str) {
    eprintln!(
        "{} - cava-style raw spectrum output for polybar (fork de lookas)\n\
         \n\
         Uso: {} [opciones]\n\
         \n\
         Opciones:\n\
         \x20 --bars N      numero de barras (default 8)\n\
         \x20 --levels N    valor maximo por barra, 1-9 (default 7, igual que cava)\n\
         \x20 --fps N       frames por segundo, 20-125 (default 60)\n\
         \x20 --system      audio del sistema (default; cae a microfono si falla)\n\
         \x20 --mic         audio del microfono\n\
         \x20 --both        mezcla sistema + microfono\n\
         \x20 --help        muestra esta ayuda\n\
         \n\
         Salida: una linea por frame con los valores separados por ';' (0..N).",
        prog, prog
    );
}

fn parse_args() -> Args {
    let mut args = Args {
        bars: 8,
        levels: 7,
        frame_ms: 16,
        mode: AudioMode::System,
    };

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bars" => {
                if let Some(v) = it.next().and_then(|v| v.parse::<usize>().ok()) {
                    args.bars = v;
                }
            }
            "--levels" => {
                if let Some(v) = it.next().and_then(|v| v.parse::<u8>().ok()) {
                    args.levels = v;
                }
            }
            "--fps" => {
                if let Some(v) = it.next().and_then(|v| v.parse::<u64>().ok()) {
                    args.frame_ms = if v > 0 { 1000 / v } else { 16 };
                }
            }
            "--mic" => args.mode = AudioMode::Mic,
            "--system" => args.mode = AudioMode::System,
            "--both" => args.mode = AudioMode::Both,
            "--help" => {
                print_usage(
                    &std::env::args()
                        .next()
                        .unwrap_or_else(|| "cava_rust".into()),
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }

    args.bars = args.bars.clamp(2, 128);
    args.levels = args.levels.clamp(1, 9);
    args.frame_ms = args.frame_ms.clamp(8, 50);
    args
}

fn init_audio(cfg: &Config, args: &Args) -> Result<AppResources> {
    let fft_size = cfg.fft_size;
    let cap = ring_cap(fft_size);
    let mic_shared = Arc::new(Mutex::new(SharedBuf::new(cap)));
    let sys_shared = Arc::new(Mutex::new(SharedBuf::new(cap)));

    let mut audio = AudioController::new();
    if audio
        .start(args.mode, mic_shared.clone(), sys_shared.clone())
        .is_err()
    {
        if matches!(args.mode, AudioMode::System) {
            audio.start(AudioMode::Mic, mic_shared.clone(), sys_shared.clone())?;
        } else {
            anyhow::bail!("no se pudo iniciar el audio en el modo pedido");
        }
    }

    let sr_u32 = audio.info().sample_rate;
    #[allow(clippy::cast_precision_loss)]
    let sr = sr_u32 as f32;

    Ok(AppResources {
        fft_size,
        audio,
        mic_shared,
        sys_shared,
        sr,
    })
}

fn init_frame(cfg: &Config, res: &AppResources, bars: usize) -> FrameState {
    let fft_size = res.fft_size;
    let half = fft_size / 2;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let buf = fft.make_input_vec();
    let fft_out = fft.make_output_vec();

    FrameState {
        analyzer: SpectrumAnalyzer::new(half),
        gate: GateState {
            pow_ema: 0.0,
            open: false,
            below_s: 0.0,
            attack_s: 0.012,
            release_s: 0.22,
            open_db: cfg.gate_db,
            close_db: (cfg.gate_db - 3.0).max(-80.0),
            confirm_s: 0.12,
        },
        window: hann(fft_size),
        half,
        fft,
        buf,
        fft_out,
        spec_pow: vec![0.0f32; half],
        mic_tail: Vec::with_capacity(fft_size),
        sys_tail: Vec::with_capacity(fft_size),
        mix: vec![0.0f32; fft_size],
        bars,
        dt_s: 0.0,
    }
}

fn tick<W: Write>(
    fs: &mut FrameState,
    res: &AppResources,
    cfg: &Config,
    levels: u8,
    out: &mut W,
) -> Result<()> {
    let mic_ok = res
        .mic_shared
        .try_lock()
        .ok()
        .is_some_and(|b| b.copy_last_n_into(res.fft_size, &mut fs.mic_tail));
    let sys_ok = res
        .sys_shared
        .try_lock()
        .ok()
        .is_some_and(|b| b.copy_last_n_into(res.fft_size, &mut fs.sys_tail));

    let tail: Option<&[f32]> = match res.audio.mode() {
        AudioMode::Mic => mic_ok.then_some(&fs.mic_tail),
        AudioMode::System => sys_ok.then_some(&fs.sys_tail),
        AudioMode::Both => {
            if !mic_ok || !sys_ok {
                None
            } else {
                #[allow(clippy::indexing_slicing)]
                for i in 0..res.fft_size {
                    fs.mix[i] = (fs.mic_tail[i] + fs.sys_tail[i]) * 0.5;
                }
                Some(&fs.mix)
            }
        }
    };

    let Some(tail) = tail else {
        return Ok(());
    };

    fs.gate.tick(compute_rms(tail, res.fft_size), fs.dt_s);

    compute_spectrum(&mut FftContext {
        tail,
        window: &fs.window,
        buf: &mut fs.buf,
        fft_out: &mut fs.fft_out,
        fft: &fs.fft,
        spec_pow: &mut fs.spec_pow,
        half: fs.half,
        fft_size: res.fft_size,
    });

    fs.analyzer
        .update_spectrum(&fs.spec_pow, cfg.tau_spec, fs.dt_s);
    fs.analyzer.analyze_bands(fs.dt_s, fs.gate.open);
    fs.analyzer.apply_flow_and_spring(
        &FlowSpringParams {
            flow_k: cfg.flow_k,
            spr_k: cfg.spr_k,
            spr_zeta: cfg.spr_zeta,
        },
        fs.dt_s,
        fs.gate.open,
    );

    let max = f32::from(levels);
    let mut line = String::with_capacity(fs.bars * 3);
    for (i, &v) in fs.analyzer.bars_y.iter().enumerate() {
        if i > 0 {
            line.push(';');
        }
        #[allow(clippy::cast_precision_loss)]
        let lvl = (v * max).round().clamp(0.0, max) as u8;
        line.push_str(&lvl.to_string());
    }
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()?;
    Ok(())
}

pub fn run() -> Result<()> {
    let args = parse_args();
    let mut cfg = Config::load()?;
    cfg.frame_ms = args.frame_ms;

    let mut out = BufWriter::with_capacity(64 * 1024, stdout());

    let res = init_audio(&cfg, &args)?;
    let mut fs = init_frame(&cfg, &res, args.bars);
    let target_dt = Duration::from_millis(cfg.frame_ms);
    let mut last = Instant::now();

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last);
        if dt < target_dt {
            if let Some(diff) = target_dt.checked_sub(dt) {
                thread::sleep(diff);
            }
        }
        let now = Instant::now();
        let dt_s = now.duration_since(last).as_secs_f32();
        last = now;

        if fs.analyzer.filters.len() != fs.bars {
            fs.analyzer.filters = build_filterbank(FilterbankParams {
                sr: res.sr,
                fft_size: res.fft_size,
                bands: fs.bars,
                fmin: cfg.fmin,
                fmax: cfg.fmax,
            });
            fs.analyzer.resize(fs.bars);
        }

        fs.dt_s = dt_s;
        tick(&mut fs, &res, &cfg, args.levels, &mut out)?;
    }
}
