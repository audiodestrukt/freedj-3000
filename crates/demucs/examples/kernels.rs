//! Kernel speeds on this machine: matrix multiplies at the transformer's and
//! the convolutions' shapes, and the attention softmax.
//!
//!   cargo run -p opendeck-demucs --release --example kernels
use opendeck_types::time::Instant;

fn gflops(m: usize, k: usize, n: usize, threads: bool) -> f64 {
    let a = vec![0.5f32; m * k];
    let b = vec![0.25f32; k * n];
    let mut c = vec![0f32; m * n];
    let run = |c: &mut [f32]| {
        if threads { opendeck_demucs::bench::pgemm(m, k, n, &a, &b, c) } else { opendeck_demucs::bench::gemm(m, k, n, &a, &b, c) }
    };
    run(&mut c);
    let t = Instant::now();
    let reps = 3;
    for _ in 0..reps { run(&mut c) }
    2.0 * (m * k * n) as f64 * reps as f64 / t.elapsed().as_secs_f64() / 1e9
}

fn main() {
    println!("threads: {}", rayon::current_num_threads());
    for (name, m, k, n) in [("ffn 2688x512x2048", 2688, 512, 2048), ("qkv 2688x512x512", 2688, 512, 512),
                            ("scores 128x64x2688", 128, 64, 2688), ("pv 128x2688x64", 128, 2688, 64),
                            ("conv 48x384x336", 48, 384, 336)] {
        println!("{name:22} 1 thread {:5.1}  all {:5.1} GFLOPS", gflops(m, k, n, false), gflops(m, k, n, true));
    }
    for (name, m, k, n) in [("ffn W·Xt 2048x512x2688", 2048, 512, 2688), ("qkv 512x512x2688", 512, 512, 2688),
                            ("conv 48x384x336", 48, 384, 336), ("conv 768x3456x336", 768, 3456, 336)] {
        let mut f = opendeck_demucs::bench::packed(m, k, n);
        f();
        let t = Instant::now();
        for _ in 0..3 { f() }
        println!("packed {name:22} {:5.1} GFLOPS", 6.0 * (m * k * n) as f64 / t.elapsed().as_secs_f64() / 1e9);
    }
    for cols in [128, 32] {
        let (e, sm) = opendeck_demucs::bench::exp_and_softmax(2688, cols);
        println!("exp {e:.2} ns/elem, softmax_cols (2688 x {cols}) {sm:.2} ns/elem");
    }
    for rows in [128, 64, 32, 24] {
        let [a, b, c] = opendeck_demucs::bench::attention_block(rows, 2688);
        let mm = 2.0 * rows as f64 * 2688.0 * 64.0 / 1e9;
        println!("attention {rows:3} queries x 2688, 1 thread: scores {:5.1} GFLOPS  softmax {:.2} ns/elem  pv {:5.1} GFLOPS  total {:.1} us/query",
                 mm / a, b * 1e9 / (rows as f64 * 2688.0), mm / c, (a + b + c) * 1e6 / rows as f64);
    }
    let mut s: Vec<f32> = (0..2688 * 2688).map(|i| ((i * 7919) % 1000) as f32 * 0.01).collect();
    let t = Instant::now();
    opendeck_demucs::bench::softmax(&mut s, 2688);
    let dt = t.elapsed().as_secs_f64();
    println!("softmax 2688x2688: {:.1} ms ({:.1} ns/exp, 1 thread); a chunk does 8.7x this", dt * 1e3, dt * 1e9 / (2688.0 * 2688.0));
}
