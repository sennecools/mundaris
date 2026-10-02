//! Contiguous bounded metadata LRU. Access sequence and address break eviction ties.
use super::cover::AddressSet;
use super::{PatchMetadata, SurfaceTopology};
use crate::RenderPreparationError;
use mundaris_math::surface::CubePatchAddress;
#[derive(Clone, Copy)]
struct Record {
    address: CubePatchAddress,
    metadata: PatchMetadata,
    sequence: u64,
}
pub(crate) struct MetadataCache {
    records: Vec<Record>,
    sequence: u64,
    limit: usize,
    pub hits: usize,
    pub misses: usize,
    pub evictions: usize,
}
impl MetadataCache {
    pub fn new(limit: usize) -> Self {
        Self {
            records: Vec::with_capacity(limit),
            sequence: 0,
            limit,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }
    pub fn reset_counters(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.evictions = 0;
    }
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.records.capacity() * std::mem::size_of::<Record>()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn get(
        &mut self,
        address: CubePatchAddress,
    ) -> Result<Option<PatchMetadata>, RenderPreparationError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(RenderPreparationError::InvalidBudget)?;
        match self.records.binary_search_by_key(&address, |r| r.address) {
            Ok(i) => {
                self.hits += 1;
                self.records[i].sequence = self.sequence;
                Ok(Some(self.records[i].metadata))
            }
            Err(_) => {
                self.misses += 1;
                Ok(None)
            }
        }
    }
    pub fn build(
        &mut self,
        address: CubePatchAddress,
        topology: &SurfaceTopology,
        pins: &AddressSet,
    ) -> Result<bool, RenderPreparationError> {
        if self
            .records
            .binary_search_by_key(&address, |r| r.address)
            .is_ok()
        {
            return Ok(true);
        }
        if self.records.len() == self.limit {
            let victim = self
                .records
                .iter()
                .enumerate()
                .filter(|(_, r)| !pins.contains(&r.address))
                .min_by_key(|(_, r)| (r.sequence, r.address))
                .map(|(i, _)| i);
            let Some(victim) = victim else {
                return Ok(false);
            };
            self.records.remove(victim);
            self.evictions += 1;
        }
        let metadata = PatchMetadata::build(address, topology)
            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(RenderPreparationError::InvalidBudget)?;
        let i = self
            .records
            .binary_search_by_key(&address, |r| r.address)
            .expect_err("new address");
        self.records.insert(
            i,
            Record {
                address,
                metadata,
                sequence: self.sequence,
            },
        );
        Ok(true)
    }
}
