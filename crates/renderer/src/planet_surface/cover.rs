//! Sorted contiguous address set; no heap-owned quadtree nodes.
use mundaris_math::surface::CubePatchAddress;
#[derive(Default, Clone)]
pub(crate) struct AddressSet {
    values: Vec<CubePatchAddress>,
}
impl AddressSet {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn clear(&mut self) {
        self.values.clear();
    }
    pub fn iter(&self) -> std::slice::Iter<'_, CubePatchAddress> {
        self.values.iter()
    }
    pub fn contains(&self, address: &CubePatchAddress) -> bool {
        self.values.binary_search(address).is_ok()
    }
    pub fn insert(&mut self, address: CubePatchAddress) -> bool {
        match self.values.binary_search(&address) {
            Ok(_) => false,
            Err(i) => {
                self.values.insert(i, address);
                true
            }
        }
    }
    pub fn remove(&mut self, address: &CubePatchAddress) -> bool {
        if let Ok(i) = self.values.binary_search(address) {
            self.values.remove(i);
            true
        } else {
            false
        }
    }
    pub fn extend(&mut self, addresses: impl IntoIterator<Item = CubePatchAddress>) {
        self.values.extend(addresses);
        self.values.sort_unstable();
        self.values.dedup();
    }
    pub fn bytes(&self) -> usize {
        self.values.capacity() * std::mem::size_of::<CubePatchAddress>()
    }
}
impl FromIterator<CubePatchAddress> for AddressSet {
    fn from_iter<I: IntoIterator<Item = CubePatchAddress>>(iter: I) -> Self {
        let mut result = Self::new();
        result.extend(iter);
        result
    }
}
impl<'a> IntoIterator for &'a AddressSet {
    type Item = &'a CubePatchAddress;
    type IntoIter = std::slice::Iter<'a, CubePatchAddress>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
