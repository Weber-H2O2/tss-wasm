/*
    Multi-party ECDSA

    Copyright 2018 by Kzen Networks

    This file is part of Multi-party ECDSA library
    (https://github.com/KZen-networks/multi-party-ecdsa)

    Multi-party ECDSA is free software: you can redistribute
    it and/or modify it under the terms of the GNU General Public
    License as published by the Free Software Foundation, either
    version 3 of the License, or (at your option) any later version.

    @license GPL-3.0+ <https://github.com/KZen-networks/multi-party-ecdsa/blob/master/LICENSE>
*/
use crate::curv::arithmetic::num_bigint::BigInt;
use crate::curv::arithmetic::traits::Samplable;
use crate::curv::cryptographic_primitives::proofs::sigma_dlog::{DLogProof, ProveDLog};
use crate::curv::elliptic::curves::secp256_k1::{FE, GE};
use crate::curv::elliptic::curves::traits::*;
use crate::paillier::{Add, Decrypt, Encrypt, Mul};
use crate::paillier::{DecryptionKey, EncryptionKey, Paillier, RawCiphertext, RawPlaintext};

use crate::gg_2018::party_i::PartyPrivate;
use crate::Error::{self, InvalidKey};

use crate::gg_2018::range_proofs::AliceProof;
use crate::paillier::zkproofs::DLogStatement;
use crate::paillier::Randomness;

use crate::curv::elliptic::curves::secp256_k1::{Secp256k1Point, Secp256k1Scalar};
use crate::paillier::traits::EncryptWithChosenRandomness;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageA {
    pub c: BigInt,                     // paillier encryption
    pub range_proofs: Vec<AliceProof>, // proofs (using other parties' h1,h2,N_tilde) that the plaintext is small
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageB {
    pub c: BigInt, // paillier encryption
    pub b_proof: DLogProof,
    pub beta_tag_proof: DLogProof,
}

impl MessageA {
    pub fn a(
        a: &Secp256k1Scalar,
        alice_ek: &EncryptionKey,
        dlog_statements: &[DLogStatement],
    ) -> (Self, BigInt) {
        let randomness = BigInt::sample_below(&alice_ek.n);
        let m_a = MessageA::a_with_predefined_randomness(a, alice_ek, &randomness, dlog_statements);
        (m_a, randomness)
    }

    pub fn a_with_predefined_randomness(
        a: &Secp256k1Scalar,
        alice_ek: &EncryptionKey,
        randomness: &BigInt,
        dlog_statements: &[DLogStatement],
    ) -> Self {
        let c_a = Paillier::encrypt_with_chosen_randomness(
            alice_ek,
            RawPlaintext::from(a.to_big_int()),
            &Randomness::from(randomness.clone()),
        )
        .0
        .clone()
        .into_owned();
        let alice_range_proofs = dlog_statements
            .iter()
            .map(|dlog_statement| {
                AliceProof::generate(&a.to_big_int(), &c_a, alice_ek, dlog_statement, randomness)
            })
            .collect::<Vec<AliceProof>>();

        Self {
            c: c_a,
            range_proofs: alice_range_proofs,
        }
    }
}

impl MessageB {
    pub fn b(
        b: &Secp256k1Scalar,
        alice_ek: &EncryptionKey,
        m_a: MessageA,
        dlog_statements: &[DLogStatement],
    ) -> Result<(Self, Secp256k1Scalar, BigInt, BigInt), Error> {
        let beta_tag = BigInt::sample_below(&alice_ek.n);
        let randomness = BigInt::sample_below(&alice_ek.n);
        let (m_b, beta) = MessageB::b_with_predefined_randomness(
            b,
            alice_ek,
            m_a,
            &randomness,
            &beta_tag,
            dlog_statements,
        )?;

        Ok((m_b, beta, randomness, beta_tag))
    }

    pub fn b_with_predefined_randomness(
        b: &Secp256k1Scalar,
        alice_ek: &EncryptionKey,
        m_a: MessageA,
        randomness: &BigInt,
        beta_tag: &BigInt,
        dlog_statements: &[DLogStatement],
    ) -> Result<(Self, Secp256k1Scalar), Error> {
        if m_a.range_proofs.len() != dlog_statements.len() {
            return Err(InvalidKey);
        }
        // verify proofs
        if !m_a
            .range_proofs
            .iter()
            .zip(dlog_statements)
            .map(|(proof, dlog_statement)| proof.verify(&m_a.c, alice_ek, dlog_statement))
            .all(|x| x)
        {
            return Err(InvalidKey);
        };
        let beta_tag_fe: Secp256k1Scalar = ECScalar::from(beta_tag);
        let c_beta_tag = Paillier::encrypt_with_chosen_randomness(
            alice_ek,
            RawPlaintext::from(beta_tag),
            &Randomness::from(randomness.clone()),
        );

        let b_bn = b.to_big_int();
        let b_c_a = Paillier::mul(
            alice_ek,
            RawCiphertext::from(m_a.c),
            RawPlaintext::from(b_bn),
        );
        let c_b = Paillier::add(alice_ek, b_c_a, c_beta_tag);
        let beta = FE::zero().sub(&beta_tag_fe.get_element());
        let dlog_proof_b = DLogProof::prove(b);
        let dlog_proof_beta_tag = DLogProof::prove(&beta_tag_fe);

        Ok((
            Self {
                c: c_b.0.clone().into_owned(),
                b_proof: dlog_proof_b,
                beta_tag_proof: dlog_proof_beta_tag,
            },
            beta,
        ))
    }

    pub fn verify_proofs_get_alpha(
        &self,
        dk: &DecryptionKey,
        a: &Secp256k1Scalar,
    ) -> Result<(Secp256k1Scalar, BigInt), Error> {
        let alice_share = Paillier::decrypt(dk, &RawCiphertext::from(self.c.clone()));
        let g: GE = ECPoint::generator();
        let alpha: FE = ECScalar::from(&alice_share.0);
        let g_alpha = g * &alpha;
        let ba_btag = &self.b_proof.pk * a + &self.beta_tag_proof.pk;
        match DLogProof::verify(&self.b_proof).is_ok()
            && DLogProof::verify(&self.beta_tag_proof).is_ok()
            && ba_btag == g_alpha
        {
            true => Ok((alpha, alice_share.0.into_owned())),
            false => Err(InvalidKey),
        }
    }

    //  another version, supportion PartyPrivate therefore binding mta to gg18.
    //  with the regular version mta can be used in general
    pub fn verify_proofs_get_alpha_gg18(
        &self,
        private: &PartyPrivate,
        a: &FE,
    ) -> Result<FE, Error> {
        let alice_share = private.decrypt(self.c.clone());
        let g: GE = ECPoint::generator();
        let alpha: FE = ECScalar::from(&alice_share.0);
        let g_alpha = g * &alpha;
        let ba_btag = &self.b_proof.pk * a + &self.beta_tag_proof.pk;

        match DLogProof::verify(&self.b_proof).is_ok()
            && DLogProof::verify(&self.beta_tag_proof).is_ok()
            && ba_btag.get_element() == g_alpha.get_element()
        {
            true => Ok(alpha),
            false => Err(InvalidKey),
        }
    }

    pub fn verify_b_against_public(public_gb: &GE, mta_gb: &GE) -> bool {
        public_gb.get_element() == mta_gb.get_element()
    }
}
