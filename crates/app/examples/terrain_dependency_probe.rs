//! Topology-only local split/balance closure widths, independent of terrain cost.
use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_math::{Direction3, surface::*};
use mundaris_renderer::planet_surface::*;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("usage: terrain_dependency_probe OUTPUT.csv")?,
    );
    let topology = SurfaceTopology::new();
    let settings = LodSettings::default()
        .with_work_limit(4096)?
        .with_limits(4096, 4096, 30)?;
    let mut csv = String::from(
        "region,source_lod,destination_lod,generated_dependencies,balance_children,retired_parents,stitch_mask_changed_neighbors,cover_leaves\n",
    );
    for (name, direction) in [
        (
            "interior",
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625),
        ),
        ("edge", DVec3::new(1.0, 0.0, 1.0)),
        ("corner", DVec3::ONE),
    ] {
        let (face, uv) = SurfaceLocation::new(Direction3::try_new(direction)?).face_uv();
        let mut leaves: BTreeSet<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut session = SurfaceLodSession::new(4096)?;
        for level in 0..25 {
            let old = active_surface_cover(&leaves.iter().copied().collect::<Vec<_>>(), &topology)?;
            session.restore_published_cover(&old)?;
            let count = 1u32 << level;
            let coordinate =
                |v: f64| (((v + 1.0) * 0.5 * f64::from(count)).floor() as u32).min(count - 1);
            let parent =
                CubePatchAddress::try_new(face, level, coordinate(uv[0]), coordinate(uv[1]))?;
            ensure!(
                leaves.contains(&parent),
                "local chain lost its covering leaf"
            );
            let dependencies = session.replacement_dependencies(parent, &settings)?;
            let mut retired = BTreeSet::new();
            for &dependency in &dependencies {
                let mut ancestor = dependency.parent();
                while let Some(a) = ancestor {
                    if leaves.contains(&a) {
                        retired.insert(a);
                        break;
                    }
                    ancestor = a.parent();
                }
            }
            for address in &retired {
                leaves.remove(address);
            }
            leaves.extend(dependencies.iter().copied());
            let new = active_surface_cover(&leaves.iter().copied().collect::<Vec<_>>(), &topology)?;
            let stitch_neighbors = new
                .iter()
                .filter(|n| {
                    old.binary_search_by_key(&n.address, |p| p.address)
                        .is_ok_and(|i| old[i].stitch_mask != n.stitch_mask)
                })
                .count();
            csv.push_str(&format!(
                "{name},{level},{},{},{},{},{stitch_neighbors},{}\n",
                level + 1,
                dependencies.len(),
                dependencies.len().saturating_sub(4),
                retired.len(),
                leaves.len()
            ));
            ensure!(
                leaves.len() <= 4096 && dependencies.len() <= 256,
                "graded balance unexpectedly exploded"
            );
        }
    }
    fs::write(output, csv)?;
    Ok(())
}
