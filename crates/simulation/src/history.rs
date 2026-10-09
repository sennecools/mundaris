//! Whole-snapshot preallocated ring. Baseline storage is separate from retention.
use crate::SimulationError;
use astrum_world::BodyState;

pub(crate) struct History {
    states: Box<[BodyState]>,
    ticks: Box<[u64]>,
    bodies: usize,
    head: usize,
    len: usize,
}
impl History {
    pub(crate) fn new(
        seed: &[BodyState],
        max_ticks: usize,
        max_bytes: usize,
    ) -> Result<Self, SimulationError> {
        let per_tick = seed
            .len()
            .checked_mul(std::mem::size_of::<BodyState>())
            .and_then(|n| n.checked_add(8))
            .ok_or(SimulationError::InvalidConfig)?;
        let usable = max_bytes
            .checked_sub(std::mem::size_of::<Self>())
            .ok_or(SimulationError::InvalidConfig)?;
        let capacity = max_ticks.min(usable / per_tick);
        if capacity < 2 {
            return Err(SimulationError::InvalidConfig);
        }
        // Exact boxed layout accounts for actual BodyState padding, tick slots and ring metadata.
        let mut states = Vec::with_capacity(capacity * seed.len());
        for _ in 0..capacity {
            states.extend_from_slice(seed);
        }
        Ok(Self {
            states: states.into_boxed_slice(),
            ticks: vec![0; capacity].into_boxed_slice(),
            bodies: seed.len(),
            head: 0,
            len: 0,
        })
    }
    pub(crate) fn capacity(&self) -> usize {
        self.ticks.len()
    }
    pub(crate) fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(&*self.states)
            + std::mem::size_of_val(&*self.ticks)
    }
    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
    pub(crate) fn push(&mut self, tick: u64, states: &[BodyState]) {
        assert_eq!(
            states.len(),
            self.bodies,
            "history records synchronized complete ticks"
        );
        let index = (self.head + self.len) % self.capacity();
        self.states[index * self.bodies..(index + 1) * self.bodies].copy_from_slice(states);
        self.ticks[index] = tick;
        if self.len == self.capacity() {
            self.head = (self.head + 1) % self.capacity();
        } else {
            self.len += 1;
        }
    }
    pub(crate) fn get(&self, tick: u64) -> Option<&[BodyState]> {
        (0..self.len)
            .map(|i| (self.head + i) % self.capacity())
            .find(|&i| self.ticks[i] == tick)
            .map(|i| &self.states[i * self.bodies..(i + 1) * self.bodies])
    }
    pub(crate) fn discard_after(&mut self, tick: u64) {
        while self.len > 0 && self.ticks[(self.head + self.len - 1) % self.capacity()] > tick {
            self.len -= 1;
        }
    }
    pub(crate) fn range(&self) -> Option<(u64, u64)> {
        (self.len > 0).then(|| {
            (
                self.ticks[self.head],
                self.ticks[(self.head + self.len - 1) % self.capacity()],
            )
        })
    }
}
