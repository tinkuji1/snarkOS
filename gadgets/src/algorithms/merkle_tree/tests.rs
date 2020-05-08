use crate::{
    algorithms::{crh::PedersenCompressedCRHGadget, merkle_tree::*},
    curves::edwards_bls12::EdwardsBlsGadget,
    define_merkle_tree_with_height,
};
use snarkos_algorithms::{
    crh::{PedersenCompressedCRH, PedersenSize},
    merkle_tree::{MerkleParameters, MerkleTree},
    prf::blake2s::Blake2s,
};
use snarkos_curves::edwards_bls12::{EdwardsProjective as Edwards, Fq};
use snarkos_models::{
    algorithms::{CRH, PRF},
    gadgets::{
        algorithms::CRHGadget,
        curves::field::FieldGadget,
        r1cs::{ConstraintSystem, TestConstraintSystem},
        utilities::{alloc::AllocGadget, uint8::UInt8},
    },
    storage::Storage,
};
use snarkos_utilities::bytes::{FromBytes, ToBytes};

use rand::{Rng, RngCore, SeedableRng};
use rand_xorshift::XorShiftRng;
use std::{
    io::{Read, Result as IoResult, Write},
    path::PathBuf,
};

#[derive(Clone)]
pub(super) struct Size;
impl PedersenSize for Size {
    const NUM_WINDOWS: usize = 256;
    const WINDOW_SIZE: usize = 4;
}

type H = PedersenCompressedCRH<Edwards, Size>;
type HG = PedersenCompressedCRHGadget<Edwards, Fq, EdwardsBlsGadget>;

define_test_merkle_tree_with_height!(EdwardsMerkleParameters, 32);
define_test_merkle_tree_with_height!(EdwardsMaskedMerkleParameters, 3);

fn generate_merkle_tree(leaves: &[[u8; 32]], use_bad_root: bool) -> () {
    type EdwardsMerkleTree = MerkleTree<EdwardsMerkleParameters>;

    let mut rng = XorShiftRng::seed_from_u64(9174123u64);

    let parameters = EdwardsMerkleParameters::setup(&mut rng);
    let tree = EdwardsMerkleTree::new(parameters.clone(), leaves).unwrap();
    let root = tree.root();
    let mut satisfied = true;
    for (i, leaf) in leaves.iter().enumerate() {
        let mut cs = TestConstraintSystem::<Fq>::new();
        let proof = tree.generate_proof(i, &leaf).unwrap();
        assert!(proof.verify(&root, &leaf).unwrap());

        // Allocate Merkle tree root
        let root = <HG as CRHGadget<H, _>>::OutputGadget::alloc(&mut cs.ns(|| format!("new_digest_{}", i)), || {
            if use_bad_root {
                Ok(<H as CRH>::Output::default())
            } else {
                Ok(root)
            }
        })
        .unwrap();

        let constraints_from_digest = cs.num_constraints();
        println!("constraints from digest: {}", constraints_from_digest);

        // Allocate Parameters for CRH
        let crh_parameters =
            <HG as CRHGadget<H, Fq>>::ParametersGadget::alloc(&mut cs.ns(|| format!("new_parameters_{}", i)), || {
                Ok(parameters.parameters())
            })
            .unwrap();

        let constraints_from_parameters = cs.num_constraints() - constraints_from_digest;
        println!("constraints from parameters: {}", constraints_from_parameters);

        // Allocate Leaf
        let leaf_g = UInt8::constant_vec(leaf);

        let constraints_from_leaf = cs.num_constraints() - constraints_from_parameters - constraints_from_digest;
        println!("constraints from leaf: {}", constraints_from_leaf);

        // Allocate Merkle tree path
        let cw =
            MerklePathGadget::<_, HG, _>::alloc(&mut cs.ns(|| format!("new_witness_{}", i)), || Ok(proof)).unwrap();

        let constraints_from_path =
            cs.num_constraints() - constraints_from_parameters - constraints_from_digest - constraints_from_leaf;
        println!("constraints from path: {}", constraints_from_path);
        let leaf_g: &[UInt8] = leaf_g.as_slice();
        cw.check_membership(
            &mut cs.ns(|| format!("new_witness_check_{}", i)),
            &crh_parameters,
            &root,
            &leaf_g,
        )
        .unwrap();
        if !cs.is_satisfied() {
            satisfied = false;
            println!("Unsatisfied constraint: {}", cs.which_is_unsatisfied().unwrap());
        }
        let setup_constraints =
            constraints_from_leaf + constraints_from_digest + constraints_from_parameters + constraints_from_path;
        println!("number of constraints: {}", cs.num_constraints() - setup_constraints);
    }

    assert!(satisfied);
}

fn generate_masked_merkle_tree(leaves: &[[u8; 32]], use_bad_root: bool) -> () {
    type EdwardsMaskedMerkleTree = MerkleTree<EdwardsMaskedMerkleParameters>;

    let mut rng = XorShiftRng::seed_from_u64(9174123u64);

    let parameters = EdwardsMaskedMerkleParameters::setup(&mut rng);
    let tree = EdwardsMaskedMerkleTree::new(parameters.clone(), leaves).unwrap();
    let root = tree.root();

    let mut cs = TestConstraintSystem::<Fq>::new();
    let leaf_gadgets = leaves.iter().map(|l| UInt8::constant_vec(l)).collect::<Vec<_>>();

    let mut nonce = [1u8; 32];
    rng.fill_bytes(&mut nonce);
    let mut root_bytes = [1u8; 32];
    root.write(&mut root_bytes[..]).unwrap();

    let mask = Blake2s::evaluate(&nonce, &root_bytes).unwrap();
    let mask_bytes = UInt8::alloc_vec(cs.ns(|| "mask"), &mask[..]).unwrap();

    let crh_parameters = <HG as CRHGadget<H, Fq>>::ParametersGadget::alloc(&mut cs.ns(|| "new_parameters"), || {
        Ok(parameters.parameters())
    })
    .unwrap();

    let computed_root = compute_root::<H, HG, _, _, _>(
        cs.ns(|| "compute masked root"),
        &crh_parameters,
        &mask_bytes,
        &leaf_gadgets,
    )
    .unwrap();

    if !cs.is_satisfied() {
        println!("Unsatisfied constraint: {}", cs.which_is_unsatisfied().unwrap());
    }
    assert!(cs.is_satisfied());
    let given_root = if use_bad_root {
        <H as CRH>::Output::default()
    } else {
        root
    };
    assert_eq!(given_root, computed_root.get_value().unwrap());
}

#[test]
fn good_root_test() {
    let mut leaves = Vec::new();
    for i in 0..4u8 {
        let input = [i; 32];
        leaves.push(input);
    }
    generate_merkle_tree(&leaves, false);
}

#[should_panic]
#[test]
fn bad_root_test() {
    let mut leaves = Vec::new();
    for i in 0..4u8 {
        let input = [i; 32];
        leaves.push(input);
    }
    generate_merkle_tree(&leaves, true);
}

#[test]
fn good_masked_root_test() {
    let mut leaves = Vec::new();
    for i in 0..4u8 {
        let input = [i; 32];
        leaves.push(input);
    }
    generate_masked_merkle_tree(&leaves, false);
}

#[should_panic]
#[test]
fn bad_masked_root_test() {
    let mut leaves = Vec::new();
    for i in 0..4u8 {
        let input = [i; 32];
        leaves.push(input);
    }
    generate_masked_merkle_tree(&leaves, true);
}
