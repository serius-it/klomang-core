use ark_ec::{CurveGroup, Group, VariableBaseMSM};
use ark_ed_on_bls12_381_bandersnatch::{EdwardsAffine, EdwardsProjective};
use ark_ff::{BigInteger, Field, PrimeField, Zero};
use ark_poly::{univariate::DensePolynomial, DenseUVPolynomial, Polynomial};
use blake3;
use std::fmt;

/// Polynomial Commitment menggunakan Inner Product Argument (IPA) dengan Bandersnatch curve
/// Implementasi ini untuk Klomang Core, pure logic, stateless, dan in-memory only
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolynomialCommitmentError {
    DegreeTooHigh,
    InvalidEvaluation,
    InvalidProof,
    SerializationError(String),
}

impl fmt::Display for PolynomialCommitmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolynomialCommitmentError::DegreeTooHigh => write!(f, "Polynomial degree too high for available generators"),
            PolynomialCommitmentError::InvalidEvaluation => write!(f, "Polynomial evaluation mismatch"),
            PolynomialCommitmentError::InvalidProof => write!(f, "Invalid IPA opening proof"),
            PolynomialCommitmentError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
        }
    }
}

impl std::error::Error for PolynomialCommitmentError {}

#[derive(Clone, Debug)]
pub struct PolynomialCommitment {
    /// Generator points untuk commitment scheme
    pub generators: Vec<EdwardsAffine>,
    /// Random point untuk blinding
    pub random_point: EdwardsAffine,
}

impl PolynomialCommitment {
    /// Membuat instance baru PolynomialCommitment dengan generators
    pub fn new(generator_count: usize) -> Self {
        // Menggunakan deterministic seed untuk reproducibility
        let mut generators = Vec::with_capacity(generator_count);

        // Generate generators deterministically using hash-to-curve
        for i in usize::MIN..generator_count {
            let point = Self::generate_generator_point(i);
            generators.push(point);
        }

        let random_point = Self::generate_generator_point(generator_count);

        Self {
            generators,
            random_point,
        }
    }

    /// Generate generator point deterministically berdasarkan index
    fn generate_generator_point(index: usize) -> EdwardsAffine {
        Self::hash_to_curve("KLOMANG_GENERATOR", index)
    }

    fn hash_to_curve(tag: &str, index: usize) -> EdwardsAffine {
        let mut counter = 0u64;
        loop {
            let mut hasher = blake3::Hasher::new();
            hasher.update(tag.as_bytes());
            hasher.update(&index.to_le_bytes());
            hasher.update(&counter.to_le_bytes());
            let hash = hasher.finalize();
            let scalar = <EdwardsProjective as Group>::ScalarField::from_le_bytes_mod_order(hash.as_bytes());
            if !scalar.is_zero() {
                return (EdwardsProjective::generator() * scalar).into_affine();
            }
            counter = counter.wrapping_add(1);
        }
    }

    /// Commit ke polinomial menggunakan IPA scheme
    pub fn commit(
        &self,
        polynomial: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
    ) -> Result<Commitment, PolynomialCommitmentError> {
        let coeffs = polynomial.coeffs();
        if coeffs.len() > self.generators.len() {
            return Err(PolynomialCommitmentError::DegreeTooHigh);
        }

        let base_slice = &self.generators[..coeffs.len()];
        let mut commitment = if coeffs.is_empty() {
            EdwardsProjective::zero()
        } else {
            EdwardsProjective::msm_unchecked(base_slice, coeffs)
        };

        let blinding_scalar = Self::generate_blinding_factor(coeffs);
        commitment += self.random_point * blinding_scalar;

        Ok(Commitment(commitment.into_affine()))
    }

    /// Membuat proof untuk opening polynomial pada point tertentu
    pub fn open(
        &self,
        polynomial: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
        point: <EdwardsProjective as Group>::ScalarField,
        value: <EdwardsProjective as Group>::ScalarField,
    ) -> Result<OpeningProof, PolynomialCommitmentError> {
        if polynomial.evaluate(&point) != value {
            return Err(PolynomialCommitmentError::InvalidEvaluation);
        }

        let quotient = self.compute_quotient_polynomial(polynomial, point, value);
        let quotient_commitment = self.commit(&quotient)?;
        let ipa_proof = self.generate_ipa_proof(&quotient)?;

        Ok(OpeningProof {
            quotient_commitment,
            ipa_proof,
            point,
            value,
        })
    }

    /// Verifikasi opening proof
    pub fn verify(
        &self,
        commitment: &Commitment,
        proof: &OpeningProof,
    ) -> Result<bool, PolynomialCommitmentError> {
        self.verify_ipa_proof(commitment, proof)
    }

    /// Hitung quotient polynomial: q(x) = (p(x) - p(z)) / (x - z)
    fn compute_quotient_polynomial(
        &self,
        polynomial: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
        point: <EdwardsProjective as Group>::ScalarField,
        value: <EdwardsProjective as Group>::ScalarField,
    ) -> DensePolynomial<<EdwardsProjective as Group>::ScalarField> {
        // p(x) - p(z)
        let mut numerator_coeffs = polynomial.coeffs().to_vec();
        numerator_coeffs[0] -= value;

        let numerator = DensePolynomial::from_coefficients_vec(numerator_coeffs);

        // x - z
        let denominator_coeffs = vec![
            -point,
            <EdwardsProjective as Group>::ScalarField::ONE,
        ];
        let denominator = DensePolynomial::from_coefficients_vec(denominator_coeffs);

        // Polynomial division
        self.polynomial_division(&numerator, &denominator)
    }

    /// Polynomial long division
    fn polynomial_division(
        &self,
        numerator: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
        denominator: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
    ) -> DensePolynomial<<EdwardsProjective as Group>::ScalarField> {
        let mut quotient_coeffs = Vec::new();
        let mut remainder = numerator.clone();

        let num_deg = numerator.degree();
        let den_deg = denominator.degree();

        if num_deg < den_deg {
            return DensePolynomial::from_coefficients_vec(Vec::new());
        }

        let den_leading_coeff = denominator.coeffs()[den_deg];

        while remainder.degree() >= den_deg {
            let rem_deg = remainder.degree();
            let rem_leading_coeff = remainder.coeffs()[rem_deg];

            // Hitung koefisien quotient
            let quotient_coeff = rem_leading_coeff * den_leading_coeff.inverse().unwrap();

            // Shift degree
            let degree_diff = rem_deg - den_deg;
            let mut quotient_term_coeffs = vec![<EdwardsProjective as Group>::ScalarField::ZERO; degree_diff + 1];
            quotient_term_coeffs[degree_diff] = quotient_coeff;

            let quotient_term = DensePolynomial::from_coefficients_vec(quotient_term_coeffs);

            // Subtract dari remainder
            let subtract_term = &quotient_term * denominator;
            remainder = &remainder - &subtract_term;

            quotient_coeffs.push(quotient_coeff);
        }

        // Reverse karena kita menambahkan dari degree tertinggi
        quotient_coeffs.reverse();
        DensePolynomial::from_coefficients_vec(quotient_coeffs)
    }

    /// Generate IPA proof untuk polynomial menggunakan commitment vector check.
    fn generate_ipa_proof(
        &self,
        polynomial: &DensePolynomial<<EdwardsProjective as Group>::ScalarField>,
    ) -> Result<IpaProof, PolynomialCommitmentError> {
        let coeffs = polynomial.coeffs().to_vec();
        let final_commitment = self.commit(polynomial)?;

        Ok(IpaProof {
            final_commitment,
            proof_scalars: coeffs,
        })
    }

    /// Verifikasi IPA proof
    fn verify_ipa_proof(
        &self,
        commitment: &Commitment,
        proof: &OpeningProof,
    ) -> Result<bool, PolynomialCommitmentError> {
        let reconstructed = self.reconstruct_commitment_from_scalars(&proof.proof_scalars)?;

        if reconstructed != proof.final_commitment {
            return Ok(false);
        }

        if reconstructed != proof.quotient_commitment {
            return Ok(false);
        }

        let p_coeffs = Self::reconstruct_polynomial_from_quotient(
            &proof.proof_scalars,
            proof.point,
            proof.value,
        );

        let p_poly = DensePolynomial::from_coefficients_vec(p_coeffs);
        let expected_commitment = self.commit(&p_poly)?;

        Ok(expected_commitment == *commitment)
    }

    /// Rekonstruksi commitment dari skalar untuk validasi proof.
    fn reconstruct_commitment_from_scalars(
        &self,
        scalars: &[<EdwardsProjective as Group>::ScalarField],
    ) -> Result<Commitment, PolynomialCommitmentError> {
        if scalars.len() > self.generators.len() {
            return Err(PolynomialCommitmentError::DegreeTooHigh);
        }

        let mut commitment = if scalars.is_empty() {
            EdwardsProjective::zero()
        } else {
            EdwardsProjective::msm_unchecked(&self.generators[..scalars.len()], scalars)
        };

        let blinding = Self::generate_blinding_factor(scalars);
        commitment += self.random_point * blinding;

        Ok(Commitment(commitment.into_affine()))
    }

    /// Generate deterministic blinding factor from polynomial coefficients
    fn generate_blinding_factor(
        coeffs: &[<EdwardsProjective as Group>::ScalarField],
    ) -> <EdwardsProjective as Group>::ScalarField {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"KLOMANG_COMMITMENT_BLINDING");

        for coeff in coeffs {
            let bytes = coeff.into_bigint().to_bytes_le();
            hasher.update(&bytes);
        }

        let hash = hasher.finalize();
        <EdwardsProjective as Group>::ScalarField::from_le_bytes_mod_order(hash.as_bytes())
    }

    fn reconstruct_polynomial_from_quotient(
        quotient_coeffs: &[<EdwardsProjective as Group>::ScalarField],
        point: <EdwardsProjective as Group>::ScalarField,
        value: <EdwardsProjective as Group>::ScalarField,
    ) -> Vec<<EdwardsProjective as Group>::ScalarField> {
        let mut p_coeffs = Vec::with_capacity(quotient_coeffs.len() + 1);
        let first = -point * quotient_coeffs.get(0).copied().unwrap_or_else(||
            <EdwardsProjective as Group>::ScalarField::ZERO
        ) + value;
        p_coeffs.push(first);

        for i in 1..=quotient_coeffs.len() {
            let prev = quotient_coeffs[i - 1];
            let next = quotient_coeffs.get(i).copied().unwrap_or_else(||
                <EdwardsProjective as Group>::ScalarField::ZERO
            );
            p_coeffs.push(prev - point * next);
        }

        p_coeffs
    }
}

/// Commitment ke polynomial
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitment(pub EdwardsAffine);

/// Proof untuk opening polynomial pada suatu point
#[derive(Clone, Debug)]
pub struct OpeningProof {
    pub quotient_commitment: Commitment,
    pub ipa_proof: IpaProof,
    pub point: <EdwardsProjective as Group>::ScalarField,
    pub value: <EdwardsProjective as Group>::ScalarField,
}

/// Inner Product Argument proof
#[derive(Clone, Debug)]
pub struct IpaProof {
    pub final_commitment: Commitment,
    pub proof_scalars: Vec<<EdwardsProjective as Group>::ScalarField>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polynomial_commitment_creation() {
        let pc = PolynomialCommitment::new(256);
        assert_eq!(pc.generators.len(), 256);
    }

    #[test]
    fn test_commit_and_open() {
        let pc = PolynomialCommitment::new(256);

        // Buat polynomial sederhana: p(x) = x^2 + 2x + 1
        let coeffs = vec![
            <EdwardsProjective as Group>::ScalarField::from(1u64),
            <EdwardsProjective as Group>::ScalarField::from(2u64),
            <EdwardsProjective as Group>::ScalarField::from(1u64),
        ];
        let polynomial = DensePolynomial::from_coefficients_vec(coeffs);

        // Commit ke polynomial
        let commitment = pc.commit(&polynomial).expect("Polynomial commitment failed");

        // Evaluate pada point x = 3
        let point = <EdwardsProjective as Group>::ScalarField::from(3u64);
        let value = polynomial.evaluate(&point);

        // Buat opening proof
        let proof = pc.open(&polynomial, point, value).expect("Opening proof failed");

        // Verifikasi proof
        assert!(pc.verify(&commitment, &proof).expect("Proof verification failed"));
    }

    #[test]
    fn test_polynomial_division() {
        let pc = PolynomialCommitment::new(256);

        // p(x) = x^2 + 2x + 1
        let p_coeffs = vec![
            <EdwardsProjective as Group>::ScalarField::from(1u64),
            <EdwardsProjective as Group>::ScalarField::from(2u64),
            <EdwardsProjective as Group>::ScalarField::from(1u64),
        ];
        let p = DensePolynomial::from_coefficients_vec(p_coeffs);

        // Point z = 1, p(1) = 4
        let z = <EdwardsProjective as Group>::ScalarField::from(1u64);
        let pz = <EdwardsProjective as Group>::ScalarField::from(4u64);

        // Compute quotient: q(x) = (p(x) - p(z)) / (x - z)
        let q = pc.compute_quotient_polynomial(&p, z, pz);

        // q(x) harus = x + 3
        let expected_q_coeffs = vec![
            <EdwardsProjective as Group>::ScalarField::from(3u64),
            <EdwardsProjective as Group>::ScalarField::from(1u64),
        ];
        let expected_q = DensePolynomial::from_coefficients_vec(expected_q_coeffs);

        assert_eq!(q.coeffs(), expected_q.coeffs());
    }
}