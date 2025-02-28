use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crypto_bigint::{MulMod, Random, U256};
use crypto_primes::{generate_prime, is_prime};

struct PublicParams {
    g: U256,
    n: U256,
    // Hash function would be implemented as a function or closure
}

/// Structure représentant l'engagement initial
pub struct Commitment {
    pp: PublicParams,
    c1: U256,
    c2: U256,
}

#[derive(Clone)]
pub struct Proof((U256, U256), (U256, U256, U256), u32);

impl Commitment {
    /// Génère des paramètres de KeyGen
    pub fn keygen(security_param: u32) -> Self {
        let mut rng = OsRng; // Générateur aléatoire sécurisé

        let lambda = security_param;

        // Générer deux grands nombres premiers p et q pour RSA
        let p: U256 = generate_prime(lambda);
        let q: U256 = generate_prime(lambda);
        // let n = p * q; // Modulus RSA
        let n = (p - U256::from_u8(1)) * (q - U256::from_u8(1)); // phi(n) = (p-1)(q-1)

        // Choisir un élément aléatoire g dans le groupe
        let g = U256::random(&mut rng) % n; // g ∈ [2, N)

        // Retourner les paramètres publics et l'engagement initial
        Self {
            pp: PublicParams { g, n },
            c1: U256::from(1u32),
            c2: g,
        }
    }

    /// Insère une nouvelle paire (k, v) dans l'engagement
    pub fn insert(&mut self, key: &str, value: &U256) -> Proof {
        let proof = Proof(
            (self.c1, self.c2),
            (self.pp.g, U256::from_u32(1), U256::from_u32(1)),
            0,
        );

        self.update_commitment(key, value);

        proof
    }

    /// Met à jour une clé existante en ajoutant un delta à la valeur stockée
    pub fn update(&mut self, key: &str, delta: &U256, proof: Proof) -> Proof {
        self.update_commitment(key, delta);
        self.proof_update(proof, key)
    }

    pub fn update_commitment(&mut self, key: &str, delta: &U256) {
        // Hachage de la clé en nombre premier
        let z = self.hash_to_prime(key);

        let c1_pow_z = self.pow_mod(self.c1, z);
        let c2_pow_value = self.pow_mod(self.c2, *delta);

        let c1_prime = <U256 as MulMod>::mul_mod(&c1_pow_z, &c2_pow_value, &self.pp.n);
        let c2_prime = self.pow_mod(self.c2, z);

        // Mise à jour de l'engagement : C1 = C1^z * C2^v mod n
        self.c1 = c1_prime;
        self.c2 = c2_prime;
    }

    fn proof_update(&self, proof: Proof, key: &str) -> Proof {
        let Proof((lambda_k1, lambda_k2), (lambda_k3, lambda_k4, lambda_k5), uk) = proof;

        // TODO: optim: éviter de regénérer z
        let z = self.hash_to_prime(key);

        Proof(
            (lambda_k1, self.pow_mod(lambda_k2, z)),
            (lambda_k3, lambda_k4, lambda_k5),
            uk + 1,
        )
    }

    pub fn verify(&self, key: &str, value: &U256, proof: Proof) -> bool {
        let Proof((lambda_k1, lambda_k2), (lambda_k3, lambda_k4, lambda_k5), uk) = proof;

        let z = self.hash_to_prime(key);

        // 1. Vérifier (Λ_k2)^z = C2
        if self.pow_mod(lambda_k2, z) != self.c2 {
            println!("Failed on first verification");
            return false;
        }

        // 2. Vérifier (Λ_k1)^(z^(u_k + 1)) * (Λ_k2)^v = c1
        let z_incr = self.pow_mod(z, U256::from_u32(uk + 1));
        let lambda_k1_pow = self.pow_mod(lambda_k1, z_incr);
        let lambda_k2_pow = self.pow_mod(lambda_k2, *value);
        let lhs1 = lambda_k1_pow * lambda_k2_pow;
        if lhs1 != self.c1 {
            println!("Failed on second verification");
            return false;
        }

        // 3. Vérifier (Λ_k3)^(z^(u_k + 1)) = c2
        let lambda_k3_pow = self.pow_mod(lambda_k3, z_incr);
        println!("g^z: {:?}", self.pow_mod(self.pp.g, z));
        println!(
            "g^z²: {:?}",
            self.pow_mod(self.pp.g, self.pow_mod(z, U256::from_u32(2)))
        );
        println!("(g^z)^z: {:?}", self.pow_mod(self.pow_mod(self.pp.g, z), z));
        println!("############");
        if lambda_k3_pow != self.c2 {
            println!("Failed on third verification");
            return false;
        }

        // 4. Vérifier (Λ_k4)^z * (Λ_k3)^Λ_k5 = g
        let lambda_k4_pow = self.pow_mod(lambda_k4, z);
        let lambda_k3_pow = self.pow_mod(lambda_k3, lambda_k5);
        let lhs3 = <U256 as MulMod>::mul_mod(&lambda_k4_pow, &lambda_k3_pow, &self.pp.n);
        if lhs3 != self.pp.g {
            println!("Failed on fourth verification");
            return false;
        }

        true
    }

    /// Exponentiation modulaire rapide : base^exp mod n
    fn pow_mod(&self, base: U256, exp: U256) -> U256 {
        let mut result = U256::from(1u8);
        let mut base = base % self.pp.n;
        let mut exp = exp;

        while exp > U256::from(0u8) {
            if exp.bit(0).into() {
                result = <U256 as MulMod>::mul_mod(&result, &base, &self.pp.n);
            }
            base = <U256 as MulMod>::mul_mod(&base, &base, &self.pp.n);
            exp >>= 1;
        }
        result
    }

    fn hash_to_prime(&self, key: &str) -> U256 {
        let lambda = 256;
        let mut counter: i32 = 0;

        loop {
            let mut hasher = Sha256::new();
            hasher.update(key.as_bytes());
            hasher.update(counter.to_le_bytes());
            let hash = hasher.finalize();

            let mut num = U256::from_u32(0);
            for byte in hash.iter().take(lambda / 8) {
                // Prend les lambda bits
                num = (num << 8) + U256::from(*byte);
            }

            num = num % self.pp.n;

            // Vérifier si num est premier ET copremier avec n
            if is_prime(&num) && num.gcd(&self.pp.n) == U256::from_u32(1) {
                return num;
            }

            counter += 1;
        }
    }
}
