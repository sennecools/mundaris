//! Time the grower stages per species (grow, mesh, score), median of N runs.
//! cargo run -p astrum_flora --release --example grow_timing -- [species_dir] [runs]

use std::time::Instant;

use astrum_flora::{Kit, grow, load_species_dir, mesh, metrics, variant_seed};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("content/flora/species");
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let kit = Kit::builtin();
    println!("| species | grow ms | mesh ms | score ms | nodes |");
    println!("|---|---|---|---|---|");
    for sp in load_species_dir(std::path::Path::new(dir)).unwrap() {
        let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
        let mut nodes = 0;
        for r in 0..runs {
            let seed = variant_seed(&sp, r as u32 % 2);
            let t = Instant::now();
            let sk = grow::grow(&sp.genome, seed);
            a.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let lods = mesh::build_lods(&sk, &sp, &kit, seed);
            b.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let _ = (metrics::structure(&sk), metrics::silhouette(&lods[0]));
            c.push(t.elapsed().as_secs_f64() * 1e3);
            nodes = sk.nodes.len();
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        println!(
            "| {} | {:.1} | {:.1} | {:.1} | {nodes} |",
            sp.name,
            med(&mut a),
            med(&mut b),
            med(&mut c)
        );
    }
}
