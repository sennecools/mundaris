//! Rocks: every content archetype grows closed, budgeted, deterministic LODs;
//! weathering rounds rocks (fewer sharp features → fewer triangles at LOD0).

use astrum_flora::hash::{derive, name_key};
use astrum_flora::rock::{Weathering, closed_edges_fraction, grow_rock, load_rocks_dir};

fn rocks() -> Vec<astrum_flora::rock::RockFile> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/rocks");
    load_rocks_dir(&dir).unwrap()
}

const DRY: Weathering = Weathering { wetness: 0.1, age: 0.2, moss: [0.05, 0.08, 0.03], moss_cover: 0.0 };
const WET: Weathering = Weathering { wetness: 0.9, age: 0.9, moss: [0.05, 0.08, 0.03], moss_cover: 0.8 };

#[test]
fn rocks_are_closed_budgeted_and_deterministic() {
    for rock in rocks() {
        for v in 0..3 {
            let seed = derive(name_key(&rock.name), v);
            for w in [DRY, WET] {
                let a = grow_rock(&rock, seed, &w);
                let b = grow_rock(&rock, seed, &w);
                assert_eq!(a[0].to_bytes(), b[0].to_bytes(), "{} v{v}", rock.name);
                let tris: Vec<usize> = a.iter().map(|m| m.triangles()).collect();
                assert!(tris[0] <= 3000 && tris[0] >= tris[1] && tris[1] >= tris[2] && tris[2] > 0, "{} {tris:?}", rock.name);
                let closed = closed_edges_fraction(&a[0]);
                assert!(closed >= 0.98, "{} v{v}: closed {closed}", rock.name);
                let (lo, _) = a[0].bounds();
                assert!(lo.z < 0.0, "{}: rock base sits in the ground", rock.name);
            }
        }
    }
}
