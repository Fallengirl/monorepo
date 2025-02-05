use crate::mmr::hasher::Hasher;
use crate::mmr::iterator::PeakIterator;
use commonware_cryptography::{Digest, Hasher as CHasher};

#[derive(Clone, Debug, PartialEq, Eq)]
/// A Proof contains the information necessary for proving the inclusion of an element, or some
/// range of elements, in the MMR.
pub struct Proof {
    pub size: u64, // total # of nodes in the MMR
    pub hashes: Vec<Digest>,
}

impl Proof {
    /// Return true if `proof` proves that `element` appears at position `element_pos` within the MMR
    /// with root hash `root_hash`.
    pub fn verify_element_inclusion<H: CHasher>(
        &self,
        element: &Digest,
        element_pos: u64,
        root_hash: &Digest,
        hasher: &mut H,
    ) -> bool {
        self.verify_range_inclusion(
            &[element.clone()],
            element_pos,
            element_pos,
            root_hash,
            hasher,
        )
    }

    /// Return true if `proof` proves that the `elements` appear consecutively between positions
    /// `start_element_pos` through `end_element_pos` (inclusive) within the MMR with root hash
    /// `root_hash`.
    pub fn verify_range_inclusion<H: CHasher>(
        &self,
        elements: &[Digest],
        start_element_pos: u64,
        end_element_pos: u64,
        root_hash: &Digest,
        hasher: &mut H,
    ) -> bool {
        let mut proof_hashes_iter = self.hashes.iter();
        let mut elements_iter = elements.iter();
        let mut siblings_iter = self.hashes.iter().rev();
        let mut mmr_hasher = Hasher::<H>::new(hasher);

        // Include peak hashes only for trees that have no elements from the range, and keep track of
        // the starting and ending trees of those that do contain some.
        let mut peak_hashes: Vec<Digest> = Vec::new();
        let mut proof_hashes_used = 0;
        for (peak_pos, height) in PeakIterator::new(self.size) {
            let leftmost_pos = peak_pos + 2 - (1 << (height + 1));
            if peak_pos >= start_element_pos && leftmost_pos <= end_element_pos {
                match peak_hash_from_range(
                    peak_pos,
                    1 << height,
                    start_element_pos,
                    end_element_pos,
                    &mut elements_iter,
                    &mut siblings_iter,
                    &mut mmr_hasher,
                ) {
                    Ok(peak_hash) => peak_hashes.push(peak_hash),
                    Err(_) => return false, // missing hashes
                }
            } else if let Some(hash) = proof_hashes_iter.next() {
                proof_hashes_used += 1;
                peak_hashes.push(hash.clone());
            } else {
                return false;
            }
        }

        if elements_iter.next().is_some() {
            return false; // some elements were not used in the proof
        }
        let next_sibling = siblings_iter.next();
        if (proof_hashes_used == 0 && next_sibling.is_some())
            || (next_sibling.is_some()
                && *next_sibling.unwrap() != self.hashes[proof_hashes_used - 1])
        {
            // some proof data was not used during verification, so we must return false to prevent
            // proof malleability attacks.
            return false;
        }
        *root_hash == mmr_hasher.root_hash(self.size, peak_hashes.iter())
    }
}

fn peak_hash_from_range<'a, H: CHasher>(
    node_pos: u64,      // current node position in the tree
    two_h: u64,         // 2^height of the current node
    leftmost_pos: u64,  // leftmost leaf in the tree to be traversed
    rightmost_pos: u64, // rightmost leaf in the tree to be traversed
    elements: &mut impl Iterator<Item = &'a Digest>,
    sibling_hashes: &mut impl Iterator<Item = &'a Digest>,
    hasher: &mut Hasher<H>,
) -> Result<Digest, ()> {
    assert_ne!(two_h, 0);
    if two_h == 1 {
        // we are at a leaf
        match elements.next() {
            Some(element) => return Ok(hasher.leaf_hash(node_pos, element)),
            None => return Err(()),
        }
    }

    let left_pos = node_pos - two_h;
    let mut left_hash: Option<Digest> = None;
    let right_pos = left_pos + two_h - 1;
    let mut right_hash: Option<Digest> = None;

    if left_pos >= leftmost_pos {
        // Descend left
        match peak_hash_from_range(
            left_pos,
            two_h >> 1,
            leftmost_pos,
            rightmost_pos,
            elements,
            sibling_hashes,
            hasher,
        ) {
            Ok(h) => left_hash = Some(h.clone()),
            Err(_) => return Err(()),
        }
    }
    if left_pos < rightmost_pos {
        // Descend right
        match peak_hash_from_range(
            right_pos,
            two_h >> 1,
            leftmost_pos,
            rightmost_pos,
            elements,
            sibling_hashes,
            hasher,
        ) {
            Ok(h) => right_hash = Some(h.clone()),
            Err(_) => return Err(()),
        }
    }

    if left_hash.is_none() {
        match sibling_hashes.next() {
            Some(hash) => left_hash = Some(hash.clone()),
            None => return Err(()),
        }
    }
    if right_hash.is_none() {
        match sibling_hashes.next() {
            Some(hash) => right_hash = Some(hash.clone()),
            None => return Err(()),
        }
    }
    Ok(hasher.node_hash(node_pos, &left_hash.unwrap(), &right_hash.unwrap()))
}

#[cfg(test)]
mod tests {
    use crate::mmr::mem::Mmr;
    use commonware_cryptography::{Digest, Hasher as CHasher, Sha256};

    #[test]
    /// Test MMR building by consecutively adding 11 equal elements to a new MMR, producing the
    /// structure in the example documented at the top of the mmr crate's mod.rs file with 19 nodes
    /// and 3 peaks.
    fn test_verify_element() {
        // create an 11 element MMR over which we'll test single-element inclusion proofs
        let mut mmr: Mmr<Sha256> = Mmr::<Sha256>::new();
        let element = Digest::from_static(b"01234567012345670123456701234567");
        let mut leaves: Vec<u64> = Vec::new();
        for _ in 0..11 {
            leaves.push(mmr.add(&element));
        }

        let root_hash = mmr.root_hash();
        let mut hasher = Sha256::default();

        // confirm the proof of inclusion for each leaf successfully verifies
        for leaf in leaves.iter().by_ref() {
            let proof = mmr.proof(*leaf);
            assert!(
                proof.verify_element_inclusion::<Sha256>(&element, *leaf, &root_hash, &mut hasher),
                "valid proof should verify successfully"
            );
        }

        // confirm mangling the proof or proof args results in failed validation
        const POS: u64 = 18;
        let proof = mmr.proof(POS);
        assert!(
            proof.verify_element_inclusion::<Sha256>(&element, POS, &root_hash, &mut hasher),
            "proof verification should be successful"
        );
        assert!(
            !proof.verify_element_inclusion::<Sha256>(&element, POS + 1, &root_hash, &mut hasher),
            "proof verification should fail with incorrect element position"
        );
        assert!(
            !proof.verify_element_inclusion::<Sha256>(&element, POS - 1, &root_hash, &mut hasher),
            "proof verification should fail with incorrect element position 2"
        );
        assert!(
            !proof.verify_element_inclusion::<Sha256>(
                &Digest::from(vec![0u8; Sha256::len()]),
                POS,
                &root_hash,
                &mut hasher
            ),
            "proof verification should fail with mangled element"
        );
        let root_hash2 = Digest::from(vec![0u8; Sha256::len()]);
        assert!(
            !proof.verify_element_inclusion::<Sha256>(&element, POS, &root_hash2, &mut hasher),
            "proof verification should fail with mangled root_hash"
        );
        let mut proof2 = proof.clone();
        proof2.hashes[0] = Digest::from(vec![0u8; Sha256::len()]);
        assert!(
            !proof2.verify_element_inclusion::<Sha256>(&element, POS, &root_hash, &mut hasher),
            "proof verification should fail with mangled proof hash"
        );
        proof2 = proof.clone();
        proof2.size = 10;
        assert!(
            !proof2.verify_element_inclusion::<Sha256>(&element, POS, &root_hash, &mut hasher),
            "proof verification should fail with incorrect size"
        );
        proof2 = proof.clone();
        proof2.hashes.push(Digest::from(vec![0u8; Sha256::len()]));
        assert!(
            !proof2.verify_element_inclusion::<Sha256>(&element, POS, &root_hash, &mut hasher),
            "proof verification should fail with extra hash"
        );
        proof2 = proof.clone();
        while !proof2.hashes.is_empty() {
            proof2.hashes.pop();
            assert!(
                !proof2.verify_element_inclusion::<Sha256>(&element, 7, &root_hash, &mut hasher),
                "proof verification should fail with missing hashes"
            );
        }
        proof2 = proof.clone();
        proof2.hashes.clear();
        const PEAK_COUNT: usize = 3;
        proof2
            .hashes
            .extend(proof.hashes[0..PEAK_COUNT - 1].iter().cloned());
        // sneak in an extra hash that won't be used in the computation and make sure it's detected
        proof2.hashes.push(Digest::from(vec![0u8; Sha256::len()]));
        proof2
            .hashes
            .extend(proof.hashes[PEAK_COUNT - 1..].iter().cloned());
        assert!(
            !proof2.verify_element_inclusion::<Sha256>(&element, POS, &root_hash, &mut hasher),
            "proof verification should fail with extra hash even if it's unused by the computation"
        );
    }

    #[test]
    fn test_verify_range() {
        // create a new MMR and add a non-trivial amount (47) of elements
        let mut mmr: Mmr<Sha256> = Mmr::default();
        let mut elements = Vec::<Digest>::new();
        let mut element_positions = Vec::<u64>::new();
        for i in 0..49 {
            elements.push(Digest::from(vec![i as u8; Sha256::len()]));
            element_positions.push(mmr.add(elements.last().unwrap()));
        }
        // test range proofs over all possible ranges of at least 2 elements
        let root_hash = mmr.root_hash();
        let mut hasher = Sha256::default();
        for i in 0..elements.len() {
            for j in i + 1..elements.len() {
                let start_pos = element_positions[i];
                let end_pos = element_positions[j];
                let range_proof = mmr.range_proof(start_pos, end_pos);
                assert!(
                    range_proof.verify_range_inclusion::<Sha256>(
                        &elements[i..j + 1],
                        start_pos,
                        end_pos,
                        &root_hash,
                        &mut hasher,
                    ),
                    "valid range proof should verify successfully {}:{}",
                    i,
                    j
                );
            }
        }

        // create a test range for which we will mangle data and confirm the proof fails
        let start_index = 33;
        let end_index = 39;
        let start_pos = element_positions[start_index];
        let end_pos = element_positions[end_index];
        let range_proof = mmr.range_proof(start_pos, end_pos);
        let valid_elements = &elements[start_index..end_index + 1];
        assert!(
            range_proof.verify_range_inclusion::<Sha256>(
                valid_elements,
                start_pos,
                end_pos,
                &root_hash,
                &mut hasher,
            ),
            "valid range proof should verify successfully"
        );
        let mut invalid_proof = range_proof.clone();
        for _i in 0..range_proof.hashes.len() {
            invalid_proof.hashes.remove(0);
            assert!(
                !range_proof.verify_range_inclusion::<Sha256>(
                    &Vec::new(),
                    start_pos,
                    end_pos,
                    &root_hash,
                    &mut hasher,
                ),
                "range proof with removed elements should fail"
            );
        }
        // confirm proof fails with invalid element hashes
        for i in 0..elements.len() {
            for j in i..elements.len() {
                assert!(
                    (i == start_index && j == end_index) // exclude the valid element range
                    || !range_proof.verify_range_inclusion::<Sha256>(
                        &elements[i..j + 1],
                        start_pos,
                        end_pos,
                        &root_hash,
                        &mut hasher,
                    ),
                    "range proof with invalid elements should fail {}:{}",
                    i,
                    j
                );
            }
        }
        // confirm proof fails with invalid root hash
        let mut invalid_root_hash = vec![0; Sha256::len()];
        invalid_root_hash[29] = root_hash[29] + 1;
        assert!(
            !range_proof.verify_range_inclusion::<Sha256>(
                valid_elements,
                start_pos,
                end_pos,
                &Digest::from(invalid_root_hash),
                &mut hasher,
            ),
            "range proof with invalid proof should fail"
        );
        // mangle the proof and confirm it fails
        let mut invalid_proof = range_proof.clone();
        invalid_proof.hashes[1] = Digest::from(vec![0u8; Sha256::len()]);
        assert!(
            !invalid_proof.verify_range_inclusion::<Sha256>(
                valid_elements,
                start_pos,
                end_pos,
                &root_hash,
                &mut hasher,
            ),
            "mangled range proof should fail verification"
        );
        // inserting elements into the proof should also cause it to fail (malleability check)
        for i in 0..range_proof.hashes.len() {
            let mut invalid_proof = range_proof.clone();
            invalid_proof
                .hashes
                .insert(i, Digest::from(vec![0u8; Sha256::len()]));
            assert!(
                !invalid_proof.verify_range_inclusion::<Sha256>(
                    valid_elements,
                    start_pos,
                    end_pos,
                    &root_hash,
                    &mut hasher,
                ),
                "mangled range proof should fail verification. inserted element at: {}",
                i
            );
        }
        // removing proof elements should cause verification to fail
        let mut invalid_proof = range_proof.clone();
        for _ in 0..range_proof.hashes.len() {
            invalid_proof.hashes.remove(0);
            assert!(
                !invalid_proof.verify_range_inclusion::<Sha256>(
                    valid_elements,
                    start_pos,
                    end_pos,
                    &root_hash,
                    &mut hasher,
                ),
                "shortened range proof should fail verification"
            );
        }
        // bad element range should cause verification to fail
        for i in 0..elements.len() {
            for j in 0..elements.len() {
                let start_pos2 = element_positions[i];
                let end_pos2 = element_positions[j];
                if start_pos2 == start_pos && end_pos2 == end_pos {
                    continue;
                }
                assert!(
                    !range_proof.verify_range_inclusion::<Sha256>(
                        valid_elements,
                        start_pos2,
                        end_pos2,
                        &root_hash,
                        &mut hasher,
                    ),
                    "bad element range should fail verification {}:{}",
                    i,
                    j
                );
            }
        }
    }
}
