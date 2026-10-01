# Architecture

## Workspace responsibilities

| Crate | Responsibility |
| --- | --- |
| `mundaris_core` | Small, dependency-light foundations shared by multiple systems when a concrete need exists. |
| `mundaris_math` | Mathematical conventions and future coordinate/reference-frame primitives; may use `glam`. |
| `mundaris_world` | Authoritative world and celestial-domain state, independent of presentation. |
| `mundaris_simulation` | Evolution of authoritative world state over time. |
| `mundaris_renderer` | GPU presentation infrastructure and disposable visual representations. |
| `mundaris_app` | Native process, event loop, logging, and top-level subsystem composition. |

Dependencies point inward toward lightweight/domain foundations. World and simulation code must not depend on `wgpu`, `winit`, or `egui`; renderer code must not own simulation truth. The app is the composition root. Avoid cycles and dependencies that exist only to make a diagram look complete.

The long-term intent is detailed in [the engine design](../MUNDARIS_ENGINE_DESIGN.md). [Phase 1](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md) specifies generic frame-tree mathematics in `mundaris_math`, future domain-owned tree instances/associations in `mundaris_world`, an app-owned validation fixture/observer session, and read-only mathematical evaluation passed to the renderer. These are planned responsibilities, not implemented APIs. Runtime frame identity remains distinct from persistent domain/generated-content identity.

## State and representation boundaries

World state is authoritative; rendering is a view of that state. Generated meshes and GPU resources are disposable caches, never the only representation of terrain or user changes. Future edits should be stored as authoritative modifications, invalidate affected derived data, and remain valid across changes in level of detail.

Future simulation coordinates that need astronomical precision should use `f64`. Local GPU-facing representations should use `f32`; conversion belongs at a clear observer/render boundary. Future local content should be expressed relative to a body's/reference frame so that body motion does not require rewriting every attached object's position. Hierarchical frames are a direction, not an API implemented here.

The planned precision path cancels shared frame ancestry and subtracts the observer in the source frame before rotation and renderer-owned narrowing. Independently flattened root positions can already have lost local detail even in `f64`. Transform ancestry carries rotation as well as translation and is distinct from orbital relationships; a moon must not inherit a planet's body spin accidentally. All evaluated poses and motion derivatives describe one coherent instant.

Future large procedural regions should be reconstructed from explicit deterministic inputs and overlaid with sparse persistent modifications. This leaves room for a global procedural surface plus sparse local volumetric changes, including edits that cannot be represented as a heightfield. Terrain, vegetation, water, atmosphere, simulation, and rendering remain independent concerns.

## Bootstrap boundary

The renderer currently only creates a native surface/device, clears and presents a frame, handles resizing, and draws a small `egui` panel. Core, world, and simulation crates intentionally contain no speculative domain types. Planet generation, terrain, reference-frame machinery, persistence, editor systems, and other engine features have not been implemented.

The app supplies the bootstrap panel through a per-frame UI callback; the renderer owns only UI input and GPU integration. Native events and UI types stay at this app/renderer boundary. An `Arc<Window>` safely keeps the surface's native handle alive without leaks or shared mutable domain state. Presentation notifies `winit` before submitting the frame to the compositor.

The smoke app uses bounded continuous redraw to exercise presentation without adding an editor repaint scheduler. Its 16 ms deadline remains stable across input wakeups, skips catch-up frames, and stops timer wakeups while minimized, zero-sized, occluded, or suspended. FIFO presentation also provides GPU/display pacing. Suspension releases the renderer before the window; resume recreates them. This bootstrap cadence is not a future simulation timestep.
