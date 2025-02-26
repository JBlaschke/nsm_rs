#include <stdint.h>
#include "params.h"
#include "poly.h"
#include "ntt.h"
#include "reduce.h"
#include "rounding.h"
#include "../../fipsmodule/sha/internal.h"

/*************************************************
* Name:        poly_reduce
*
* Description: Inplace reduction of all coefficients of polynomial to
*              representative in [-6283009,6283007].
*
* Arguments:   - poly *a: pointer to input/output polynomial
**************************************************/
void poly_reduce(poly *a) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    a->coeffs[i] = reduce32(a->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_caddq
*
* Description: For all coefficients of in/out polynomial add Q if
*              coefficient is negative.
*
* Arguments:   - poly *a: pointer to input/output polynomial
**************************************************/
void poly_caddq(poly *a) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    a->coeffs[i] = caddq(a->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_add
*
* Description: Add polynomials. No modular reduction is performed.
*
* Arguments:   - poly *c: pointer to output polynomial
*              - const poly *a: pointer to first summand
*              - const poly *b: pointer to second summand
**************************************************/
void poly_add(poly *c, const poly *a, const poly *b)  {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    c->coeffs[i] = a->coeffs[i] + b->coeffs[i];
  }
}

/*************************************************
* Name:        poly_sub
*
* Description: Subtract polynomials. No modular reduction is
*              performed.
*
* Arguments:   - poly *c: pointer to output polynomial
*              - const poly *a: pointer to first input polynomial
*              - const poly *b: pointer to second input polynomial to be
*                               subtraced from first input polynomial
**************************************************/
void poly_sub(poly *c, const poly *a, const poly *b) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    c->coeffs[i] = a->coeffs[i] - b->coeffs[i];
  }
}

/*************************************************
* Name:        poly_shiftl
*
* Description: Multiply polynomial by 2^D without modular reduction. Assumes
*              input coefficients to be less than 2^{31-D} in absolute value.
*
* Arguments:   - poly *a: pointer to input/output polynomial
**************************************************/
void poly_shiftl(poly *a) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    a->coeffs[i] <<= D;
  }
}

/*************************************************
* Name:        poly_ntt
*
* Description: Inplace forward NTT. Coefficients can grow by
*              8*Q in absolute value.
*
* Arguments:   - poly *a: pointer to input/output polynomial
**************************************************/
void poly_ntt(poly *a) {
  ntt(a->coeffs);
}

/*************************************************
* Name:        poly_invntt_tomont
*
* Description: Inplace inverse NTT and multiplication by 2^{32}.
*              Input coefficients need to be less than Q in absolute
*              value and output coefficients are again bounded by Q.
*
* Arguments:   - poly *a: pointer to input/output polynomial
**************************************************/
void poly_invntt_tomont(poly *a) {
  invntt_tomont(a->coeffs);
}

/*************************************************
* Name:        poly_pointwise_montgomery
*
* Description: Pointwise multiplication of polynomials in NTT domain
*              representation and multiplication of resulting polynomial
*              by 2^{-32}.
*
* Arguments:   - poly *c: pointer to output polynomial
*              - const poly *a: pointer to first input polynomial
*              - const poly *b: pointer to second input polynomial
**************************************************/
void poly_pointwise_montgomery(poly *c, const poly *a, const poly *b) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    c->coeffs[i] = fqmul(a->coeffs[i], b->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_power2round
*
* Description: For all coefficients c of the input polynomial,
*              compute c0, c1 such that c mod Q = c1*2^D + c0
*              with -2^{D-1} < c0 <= 2^{D-1}. Assumes coefficients to be
*              standard representatives.
*
* Arguments:   - poly *a1: pointer to output polynomial with coefficients c1
*              - poly *a0: pointer to output polynomial with coefficients c0
*              - const poly *a: pointer to input polynomial
**************************************************/
void poly_power2round(poly *a1, poly *a0, const poly *a) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    a1->coeffs[i] = power2round(&a0->coeffs[i], a->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_decompose
*
* Description: For all coefficients c of the input polynomial,
*              compute high and low bits c0, c1 such c mod Q = c1*ALPHA + c0
*              with -ALPHA/2 < c0 <= ALPHA/2 except c1 = (Q-1)/ALPHA where we
*              set c1 = 0 and -ALPHA/2 <= c0 = c mod Q - Q < 0.
*              Assumes coefficients to be standard representatives.
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *a1: pointer to output polynomial with coefficients c1
*              - poly *a0: pointer to output polynomial with coefficients c0
*              - const poly *a: pointer to input polynomial
**************************************************/
void poly_decompose(ml_dsa_params *params, poly *a1, poly *a0, const poly *a) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    a1->coeffs[i] = decompose(params, &a0->coeffs[i], a->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_make_hint
*
* Description: Compute hint polynomial. The coefficients of which indicate
*              whether the low bits of the corresponding coefficient of
*              the input polynomial overflow into the high bits.
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *h: pointer to output hint polynomial
*              - const poly *a0: pointer to low part of input polynomial
*              - const poly *a1: pointer to high part of input polynomial
*
* Returns number of 1 bits.
**************************************************/
unsigned int poly_make_hint(ml_dsa_params *params, poly *h, const poly *a0, const poly *a1) {
  unsigned int i, s = 0;
  for(i = 0; i < N; ++i) {
    h->coeffs[i] = make_hint(params, a0->coeffs[i], a1->coeffs[i]);
    s += h->coeffs[i];
  }
  return s;
}

/*************************************************
* Name:        poly_use_hint
*
* Description: Use hint polynomial to correct the high bits of a polynomial.
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *b: pointer to output polynomial with corrected high bits
*              - const poly *a: pointer to input polynomial
*              - const poly *h: pointer to input hint polynomial
**************************************************/
void poly_use_hint(ml_dsa_params *params, poly *b, const poly *a, const poly *h) {
  unsigned int i;
  for(i = 0; i < N; ++i) {
    b->coeffs[i] = use_hint(params, a->coeffs[i], h->coeffs[i]);
  }
}

/*************************************************
* Name:        poly_chknorm
*
* Description: Check infinity norm of polynomial against given bound.
*              Assumes input coefficients were reduced by reduce32().
*
* Arguments:   - const poly *a: pointer to polynomial
*              - int32_t B: norm bound
*
* Returns 0 if norm is strictly smaller than B <= (Q-1)/8 and 1 otherwise.
**************************************************/
int poly_chknorm(const poly *a, int32_t B) {
  unsigned int i;
  int32_t t;

  if(B > (Q-1)/8) {
    return 1;
  }

  /* It is ok to leak which coefficient violates the bound since
     the probability for each coefficient is independent of secret
     data but we must not leak the sign of the centralized representative. */
  for(i = 0; i < N; ++i) {
    /* Absolute value */
    t = a->coeffs[i] >> 31;
    t = a->coeffs[i] - (t & 2*a->coeffs[i]);

    if(t >= B) {
      return 1;
    }
  }
  return 0;
}

/*************************************************
* Name:        rej_uniform
*
* Description: Sample uniformly random coefficients in [0, Q-1] by
*              performing rejection sampling on array of random bytes.
*
* Arguments:   - int32_t *a: pointer to output array (allocated)
*              - unsigned int len: number of coefficients to be sampled
*              - const uint8_t *buf: array of random bytes
*              - unsigned int buflen: length of array of random bytes
*
* Returns number of sampled coefficients. Can be smaller than len if not enough
* random bytes were given.
**************************************************/
static unsigned int rej_uniform(int32_t *a,
                                unsigned int len,
                                const uint8_t *buf,
                                unsigned int buflen)
{
  unsigned int ctr, pos;
  uint32_t t;

  ctr = pos = 0;
  while(ctr < len && pos + 3 <= buflen) {
    t  = buf[pos++];
    t |= (uint32_t)buf[pos++] << 8;
    t |= (uint32_t)buf[pos++] << 16;
    t &= 0x7FFFFF;

    if(t < Q) {
      a[ctr++] = t;
    }
  }
  return ctr;
}

/*************************************************
* Name:        poly_uniform
*
* Description: FIPS 204: Algorithm 30 RejNTTPoly.
*              Sample polynomial with uniformly random coefficients
*              in [0,Q-1] by performing rejection sampling on the
*              output stream of SHAKE128(seed|nonce)
*
* Arguments:   - poly *a: pointer to output polynomial
*              - const uint8_t seed[]: byte array with seed of length SEEDBYTES
*              - uint16_t nonce: 2-byte nonce
**************************************************/
#define POLY_UNIFORM_NBLOCKS ((768 + SHAKE128_RATE - 1)/ SHAKE128_RATE)
void poly_uniform(poly *a,
                  const uint8_t seed[SEEDBYTES],
                  uint16_t nonce)
{
  unsigned int i, ctr, off;
  unsigned int buflen = POLY_UNIFORM_NBLOCKS*SHAKE128_RATE;
  uint8_t buf[POLY_UNIFORM_NBLOCKS*SHAKE128_RATE + 2];
  KECCAK1600_CTX state;

  uint8_t t[2];
  t[0] = nonce & 0xff;
  t[1] = nonce >> 8;

  SHAKE_Init(&state, SHAKE128_BLOCKSIZE);
  SHA3_Update(&state, seed, SEEDBYTES);
  SHA3_Update(&state, t, 2);
  SHAKE_Final(buf, &state, POLY_UNIFORM_NBLOCKS * SHAKE128_BLOCKSIZE);

  ctr = rej_uniform(a->coeffs, N, buf, buflen);

  while(ctr < N) {
    off = buflen % 3;
    for(i = 0; i < off; ++i)
      buf[i] = buf[buflen - off + i];

    SHAKE_Final(buf + off, &state, POLY_UNIFORM_NBLOCKS * SHAKE128_BLOCKSIZE);
    buflen = SHAKE128_RATE + off;
    ctr += rej_uniform(a->coeffs + ctr, N - ctr, buf, buflen);
  }
  /* FIPS 204. Section 3.6.3 Destruction of intermediate values. */
  OPENSSL_cleanse(buf, sizeof(buf));
  OPENSSL_cleanse(&state, sizeof(state));
}

/*************************************************
* Name:        rej_eta
*
* Description: Sample uniformly random coefficients in [-ETA, ETA] by
*              performing rejection sampling on array of random bytes.
*
* Arguments:   - ml_dsa_params: parameter struct
*              - int32_t *a: pointer to output array (allocated)
*              - unsigned int len: number of coefficients to be sampled
*              - const uint8_t *buf: array of random bytes
*              - unsigned int buflen: length of array of random bytes
*
* Returns number of sampled coefficients. Can be smaller than len if not enough
* random bytes were given.
**************************************************/
static unsigned int rej_eta(ml_dsa_params *params,
                            int32_t *a,
                            unsigned int len,
                            const uint8_t *buf,
                            unsigned int buflen)
{

  assert((params->eta == 2) ||
         (params->eta == 4));

  unsigned int ctr, pos;
  uint32_t t0, t1;

  ctr = pos = 0;
  while(ctr < len && pos < buflen) {
    t0 = buf[pos] & 0x0F;
    t1 = buf[pos++] >> 4;

    if (params->eta == 2) {
      if(t0 < 15) {
        t0 = t0 - (205*t0 >> 10)*5;
        a[ctr++] = 2 - t0;
      }
      if(t1 < 15 && ctr < len) {
        t1 = t1 - (205*t1 >> 10)*5;
        a[ctr++] = 2 - t1;
      }
    }

    else if (params->eta == 4) {
      if(t0 < 9)
        a[ctr++] = 4 - t0;
      if(t1 < 9 && ctr < len)
        a[ctr++] = 4 - t1;
    }
  }
  return ctr;
}

/*************************************************
* Name:        poly_uniform_eta
*
* Description: FIPS 204: Algorithm 31 RejBoundedPoly.
*              Sample polynomial with uniformly random coefficients
*              in [-ETA,ETA] by performing rejection sampling on the
*              output stream from SHAKE256(seed|nonce)
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *a: pointer to output polynomial
*              - const uint8_t seed[]: byte array with seed of length CRHBYTES
*              - uint16_t nonce: 2-byte nonce
**************************************************/
void poly_uniform_eta(ml_dsa_params *params,
                      poly *a,
                      const uint8_t seed[CRHBYTES],
                      uint16_t nonce)
{
  unsigned int ctr;
  unsigned int buflen = DILITHIUM_POLY_UNIFORM_ETA_NBLOCKS_MAX * SHAKE256_RATE;
  uint8_t buf[DILITHIUM_POLY_UNIFORM_ETA_NBLOCKS_MAX * SHAKE256_RATE];
  KECCAK1600_CTX state;

  uint8_t t[2];
  t[0] = nonce & 0xff;
  t[1] = nonce >> 8;

  SHAKE_Init(&state, SHAKE256_BLOCKSIZE);
  SHA3_Update(&state, seed, CRHBYTES);
  SHA3_Update(&state, t, 2);
  SHAKE_Final(buf, &state, DILITHIUM_POLY_UNIFORM_ETA_NBLOCKS_MAX * SHAKE256_BLOCKSIZE);

  ctr = rej_eta(params, a->coeffs, N, buf, buflen);

  while(ctr < N) {
    SHAKE_Final(buf, &state, SHAKE256_BLOCKSIZE);
    ctr += rej_eta(params, a->coeffs + ctr, N - ctr, buf, SHAKE256_RATE);
  }
  /* FIPS 204. Section 3.6.3 Destruction of intermediate values. */
  OPENSSL_cleanse(buf, sizeof(buf));
  OPENSSL_cleanse(&state, sizeof(state));
}

/*************************************************
* Name:        poly_uniform_gamma1
*
* Description: Sample polynomial with uniformly random coefficients
*              in [-(GAMMA1 - 1), GAMMA1] by unpacking output stream
*              of SHAKE256(seed|nonce)
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *a: pointer to output polynomial
*              - const uint8_t seed[]: byte array with seed of length CRHBYTES
*              - uint16_t nonce: 16-bit nonce
**************************************************/
#define POLY_UNIFORM_GAMMA1_NBLOCKS ((DILITHIUM_POLYZ_PACKEDBYTES_MAX + SHAKE256_RATE - 1) / SHAKE256_RATE)
void poly_uniform_gamma1(ml_dsa_params *params,
                         poly *a,
                         const uint8_t seed[CRHBYTES],
                         uint16_t nonce)
{
  uint8_t buf[POLY_UNIFORM_GAMMA1_NBLOCKS * SHAKE256_RATE];
  KECCAK1600_CTX state;

  uint8_t t[2];
  t[0] = nonce & 0xff;
  t[1] = nonce >> 8;

  SHAKE_Init(&state, SHAKE256_BLOCKSIZE);
  SHA3_Update(&state, seed, CRHBYTES);
  SHA3_Update(&state, t, 2);

  SHAKE_Final(buf, &state, POLY_UNIFORM_GAMMA1_NBLOCKS * SHAKE256_BLOCKSIZE);
  polyz_unpack(params, a, buf);
  /* FIPS 204. Section 3.6.3 Destruction of intermediate values. */
  OPENSSL_cleanse(buf, sizeof(buf));
  OPENSSL_cleanse(&state, sizeof(state));
}

/*************************************************
* Name:        poly_challenge
*
* Description: Implementation of H. Samples polynomial with TAU nonzero
*              coefficients in {-1,1} using the output stream of
*              SHAKE256(seed).
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *c: pointer to output polynomial
*              - const uint8_t mu[]: byte array containing seed of length CTILDEBYTES
**************************************************/
void poly_challenge(ml_dsa_params *params, poly *c, const uint8_t *seed) {
  unsigned int i, b, pos;
  uint64_t signs;
  uint8_t buf[SHAKE256_RATE];
  KECCAK1600_CTX state;

  SHAKE_Init(&state, SHAKE256_BLOCKSIZE);
  SHA3_Update(&state, seed, params->c_tilde_bytes);
  SHAKE_Final(buf, &state, SHAKE256_BLOCKSIZE);

  signs = 0;
  for(i = 0; i < 8; ++i) {
    signs |= (uint64_t)buf[i] << 8*i;
  }
  pos = 8;

  for(i = 0; i < N; ++i) {
    c->coeffs[i] = 0;
  }
  for(i = N-params->tau; i < N; ++i) {
    do {
      if(pos >= SHAKE256_RATE) {
        SHAKE_Final(buf, &state, SHAKE256_BLOCKSIZE);
        pos = 0;
      }

      b = buf[pos++];
    } while(b > i);

    c->coeffs[i] = c->coeffs[b];
    c->coeffs[b] = 1 - 2*(signs & 1);
    signs >>= 1;
  }
  /* FIPS 204. Section 3.6.3 Destruction of intermediate values. */
  OPENSSL_cleanse(&signs, sizeof(signs));
  OPENSSL_cleanse(buf, sizeof(buf));
  OPENSSL_cleanse(&state, sizeof(state));
}

/*************************************************
* Name:        polyeta_pack
*
* Description: Bit-pack polynomial with coefficients in [-ETA,ETA].
*
* Arguments:   - ml_dsa_params: parameter struct
*              - uint8_t *r: pointer to output byte array with at least
*                            POLYETA_PACKEDBYTES bytes
*              - const poly *a: pointer to input polynomial
**************************************************/
void polyeta_pack(ml_dsa_params *params, uint8_t *r, const poly *a) {
  unsigned int i;
  uint8_t t[8];

  assert((params->eta == 2) ||
         (params->eta == 4));

  if (params->eta == 2) {
    for(i = 0; i < N/8; ++i) {
      t[0] = params->eta - a->coeffs[8*i+0];
      t[1] = params->eta - a->coeffs[8*i+1];
      t[2] = params->eta - a->coeffs[8*i+2];
      t[3] = params->eta - a->coeffs[8*i+3];
      t[4] = params->eta - a->coeffs[8*i+4];
      t[5] = params->eta - a->coeffs[8*i+5];
      t[6] = params->eta - a->coeffs[8*i+6];
      t[7] = params->eta - a->coeffs[8*i+7];

      r[3*i+0]  = (t[0] >> 0) | (t[1] << 3) | (t[2] << 6);
      r[3*i+1]  = (t[2] >> 2) | (t[3] << 1) | (t[4] << 4) | (t[5] << 7);
      r[3*i+2]  = (t[5] >> 1) | (t[6] << 2) | (t[7] << 5);
    }
  }
  else if (params->eta == 4) {
    for(i = 0; i < N/2; ++i) {
      t[0] = params->eta - a->coeffs[2*i+0];
      t[1] = params->eta - a->coeffs[2*i+1];
      r[i] = t[0] | (t[1] << 4);
    }
  }
}

/*************************************************
* Name:        polyeta_unpack
*
* Description: Unpack polynomial with coefficients in [-ETA,ETA].
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *r: pointer to output polynomial
*              - const uint8_t *a: byte array with bit-packed polynomial
**************************************************/
void polyeta_unpack(ml_dsa_params *params, poly *r, const uint8_t *a) {
  unsigned int i;
  assert((params->eta == 2) ||
       (params->eta == 4));

  if (params->eta == 2) {
    for(i = 0; i < N/8; ++i) {
      r->coeffs[8*i+0] =  (a[3*i+0] >> 0) & 7;
      r->coeffs[8*i+1] =  (a[3*i+0] >> 3) & 7;
      r->coeffs[8*i+2] = ((a[3*i+0] >> 6) | (a[3*i+1] << 2)) & 7;
      r->coeffs[8*i+3] =  (a[3*i+1] >> 1) & 7;
      r->coeffs[8*i+4] =  (a[3*i+1] >> 4) & 7;
      r->coeffs[8*i+5] = ((a[3*i+1] >> 7) | (a[3*i+2] << 1)) & 7;
      r->coeffs[8*i+6] =  (a[3*i+2] >> 2) & 7;
      r->coeffs[8*i+7] =  (a[3*i+2] >> 5) & 7;

      r->coeffs[8*i+0] = params->eta - r->coeffs[8*i+0];
      r->coeffs[8*i+1] = params->eta - r->coeffs[8*i+1];
      r->coeffs[8*i+2] = params->eta - r->coeffs[8*i+2];
      r->coeffs[8*i+3] = params->eta - r->coeffs[8*i+3];
      r->coeffs[8*i+4] = params->eta - r->coeffs[8*i+4];
      r->coeffs[8*i+5] = params->eta - r->coeffs[8*i+5];
      r->coeffs[8*i+6] = params->eta - r->coeffs[8*i+6];
      r->coeffs[8*i+7] = params->eta - r->coeffs[8*i+7];
    }
  }
  else if (params->eta == 4) {
    for(i = 0; i < N/2; ++i) {
      r->coeffs[2*i+0] = a[i] & 0x0F;
      r->coeffs[2*i+1] = a[i] >> 4;
      r->coeffs[2*i+0] = params->eta - r->coeffs[2*i+0];
      r->coeffs[2*i+1] = params->eta - r->coeffs[2*i+1];
    }
  }
}

/*************************************************
* Name:        polyt1_pack
*
* Description: Bit-pack polynomial t1 with coefficients fitting in 10 bits.
*              Input coefficients are assumed to be standard representatives.
*
* Arguments:   - uint8_t *r: pointer to output byte array with at least
*                            POLYT1_PACKEDBYTES bytes
*              - const poly *a: pointer to input polynomial
**************************************************/
void polyt1_pack(uint8_t *r, const poly *a) {
  unsigned int i;

  for(i = 0; i < N/4; ++i) {
    r[5*i+0] = (a->coeffs[4*i+0] >> 0);
    r[5*i+1] = (a->coeffs[4*i+0] >> 8) | (a->coeffs[4*i+1] << 2);
    r[5*i+2] = (a->coeffs[4*i+1] >> 6) | (a->coeffs[4*i+2] << 4);
    r[5*i+3] = (a->coeffs[4*i+2] >> 4) | (a->coeffs[4*i+3] << 6);
    r[5*i+4] = (a->coeffs[4*i+3] >> 2);
  }
}

/*************************************************
* Name:        polyt1_unpack
*
* Description: Unpack polynomial t1 with 10-bit coefficients.
*              Output coefficients are standard representatives.
*
* Arguments:   - poly *r: pointer to output polynomial
*              - const uint8_t *a: byte array with bit-packed polynomial
**************************************************/
void polyt1_unpack(poly *r, const uint8_t *a) {
  unsigned int i;

  for(i = 0; i < N/4; ++i) {
    r->coeffs[4*i+0] = ((a[5*i+0] >> 0) | ((uint32_t)a[5*i+1] << 8)) & 0x3FF;
    r->coeffs[4*i+1] = ((a[5*i+1] >> 2) | ((uint32_t)a[5*i+2] << 6)) & 0x3FF;
    r->coeffs[4*i+2] = ((a[5*i+2] >> 4) | ((uint32_t)a[5*i+3] << 4)) & 0x3FF;
    r->coeffs[4*i+3] = ((a[5*i+3] >> 6) | ((uint32_t)a[5*i+4] << 2)) & 0x3FF;
  }
}

/*************************************************
* Name:        polyt0_pack
*
* Description: Bit-pack polynomial t0 with coefficients in ]-2^{D-1}, 2^{D-1}].
*
* Arguments:   - uint8_t *r: pointer to output byte array with at least
*                            POLYT0_PACKEDBYTES bytes
*              - const poly *a: pointer to input polynomial
**************************************************/
void polyt0_pack(uint8_t *r, const poly *a) {
  unsigned int i;
  uint32_t t[8];

  for(i = 0; i < N/8; ++i) {
    t[0] = (1 << (D-1)) - a->coeffs[8*i+0];
    t[1] = (1 << (D-1)) - a->coeffs[8*i+1];
    t[2] = (1 << (D-1)) - a->coeffs[8*i+2];
    t[3] = (1 << (D-1)) - a->coeffs[8*i+3];
    t[4] = (1 << (D-1)) - a->coeffs[8*i+4];
    t[5] = (1 << (D-1)) - a->coeffs[8*i+5];
    t[6] = (1 << (D-1)) - a->coeffs[8*i+6];
    t[7] = (1 << (D-1)) - a->coeffs[8*i+7];

    r[13*i+ 0]  =  t[0];
    r[13*i+ 1]  =  t[0] >>  8;
    r[13*i+ 1] |=  t[1] <<  5;
    r[13*i+ 2]  =  t[1] >>  3;
    r[13*i+ 3]  =  t[1] >> 11;
    r[13*i+ 3] |=  t[2] <<  2;
    r[13*i+ 4]  =  t[2] >>  6;
    r[13*i+ 4] |=  t[3] <<  7;
    r[13*i+ 5]  =  t[3] >>  1;
    r[13*i+ 6]  =  t[3] >>  9;
    r[13*i+ 6] |=  t[4] <<  4;
    r[13*i+ 7]  =  t[4] >>  4;
    r[13*i+ 8]  =  t[4] >> 12;
    r[13*i+ 8] |=  t[5] <<  1;
    r[13*i+ 9]  =  t[5] >>  7;
    r[13*i+ 9] |=  t[6] <<  6;
    r[13*i+10]  =  t[6] >>  2;
    r[13*i+11]  =  t[6] >> 10;
    r[13*i+11] |=  t[7] <<  3;
    r[13*i+12]  =  t[7] >>  5;
  }
}

/*************************************************
* Name:        polyt0_unpack
*
* Description: Unpack polynomial t0 with coefficients in ]-2^{D-1}, 2^{D-1}].
*
* Arguments:   - poly *r: pointer to output polynomial
*              - const uint8_t *a: byte array with bit-packed polynomial
**************************************************/
void polyt0_unpack(poly *r, const uint8_t *a) {
  unsigned int i;

  for(i = 0; i < N/8; ++i) {
    r->coeffs[8*i+0]  = a[13*i+0];
    r->coeffs[8*i+0] |= (uint32_t)a[13*i+1] << 8;
    r->coeffs[8*i+0] &= 0x1FFF;

    r->coeffs[8*i+1]  = a[13*i+1] >> 5;
    r->coeffs[8*i+1] |= (uint32_t)a[13*i+2] << 3;
    r->coeffs[8*i+1] |= (uint32_t)a[13*i+3] << 11;
    r->coeffs[8*i+1] &= 0x1FFF;

    r->coeffs[8*i+2]  = a[13*i+3] >> 2;
    r->coeffs[8*i+2] |= (uint32_t)a[13*i+4] << 6;
    r->coeffs[8*i+2] &= 0x1FFF;

    r->coeffs[8*i+3]  = a[13*i+4] >> 7;
    r->coeffs[8*i+3] |= (uint32_t)a[13*i+5] << 1;
    r->coeffs[8*i+3] |= (uint32_t)a[13*i+6] << 9;
    r->coeffs[8*i+3] &= 0x1FFF;

    r->coeffs[8*i+4]  = a[13*i+6] >> 4;
    r->coeffs[8*i+4] |= (uint32_t)a[13*i+7] << 4;
    r->coeffs[8*i+4] |= (uint32_t)a[13*i+8] << 12;
    r->coeffs[8*i+4] &= 0x1FFF;

    r->coeffs[8*i+5]  = a[13*i+8] >> 1;
    r->coeffs[8*i+5] |= (uint32_t)a[13*i+9] << 7;
    r->coeffs[8*i+5] &= 0x1FFF;

    r->coeffs[8*i+6]  = a[13*i+9] >> 6;
    r->coeffs[8*i+6] |= (uint32_t)a[13*i+10] << 2;
    r->coeffs[8*i+6] |= (uint32_t)a[13*i+11] << 10;
    r->coeffs[8*i+6] &= 0x1FFF;

    r->coeffs[8*i+7]  = a[13*i+11] >> 3;
    r->coeffs[8*i+7] |= (uint32_t)a[13*i+12] << 5;
    r->coeffs[8*i+7] &= 0x1FFF;

    r->coeffs[8*i+0] = (1 << (D-1)) - r->coeffs[8*i+0];
    r->coeffs[8*i+1] = (1 << (D-1)) - r->coeffs[8*i+1];
    r->coeffs[8*i+2] = (1 << (D-1)) - r->coeffs[8*i+2];
    r->coeffs[8*i+3] = (1 << (D-1)) - r->coeffs[8*i+3];
    r->coeffs[8*i+4] = (1 << (D-1)) - r->coeffs[8*i+4];
    r->coeffs[8*i+5] = (1 << (D-1)) - r->coeffs[8*i+5];
    r->coeffs[8*i+6] = (1 << (D-1)) - r->coeffs[8*i+6];
    r->coeffs[8*i+7] = (1 << (D-1)) - r->coeffs[8*i+7];
  }
}

/*************************************************
* Name:        polyz_pack
*
* Description: Bit-pack polynomial with coefficients
*              in [-(GAMMA1 - 1), GAMMA1].
*
* Arguments:   - ml_dsa_params: parameter struct
*              - uint8_t *r: pointer to output byte array with at least
*                            POLYZ_PACKEDBYTES bytes
*              - const poly *a: pointer to input polynomial
**************************************************/
void polyz_pack(ml_dsa_params *params, uint8_t *r, const poly *a) {
  unsigned int i;
  uint32_t t[4];

  assert((params->gamma1 == (1 << 17)) ||
       (params->gamma1 == (1 << 19)));

  if (params->gamma1 == (1 << 17)) {
    for(i = 0; i < N/4; ++i) {
      t[0] = params->gamma1  - a->coeffs[4*i+0];
      t[1] = params->gamma1  - a->coeffs[4*i+1];
      t[2] = params->gamma1  - a->coeffs[4*i+2];
      t[3] = params->gamma1  - a->coeffs[4*i+3];

      r[9*i+0]  = t[0];
      r[9*i+1]  = t[0] >> 8;
      r[9*i+2]  = t[0] >> 16;
      r[9*i+2] |= t[1] << 2;
      r[9*i+3]  = t[1] >> 6;
      r[9*i+4]  = t[1] >> 14;
      r[9*i+4] |= t[2] << 4;
      r[9*i+5]  = t[2] >> 4;
      r[9*i+6]  = t[2] >> 12;
      r[9*i+6] |= t[3] << 6;
      r[9*i+7]  = t[3] >> 2;
      r[9*i+8]  = t[3] >> 10;
    }
  }
  else if (params->gamma1 == (1 << 19)) {
    for(i = 0; i < N/2; ++i) {
      t[0] = params->gamma1 - a->coeffs[2*i+0];
      t[1] = params->gamma1 - a->coeffs[2*i+1];

      r[5*i+0]  = t[0];
      r[5*i+1]  = t[0] >> 8;
      r[5*i+2]  = t[0] >> 16;
      r[5*i+2] |= t[1] << 4;
      r[5*i+3]  = t[1] >> 4;
      r[5*i+4]  = t[1] >> 12;
    }
  }
}

/*************************************************
* Name:        polyz_unpack
*
* Description: Unpack polynomial z with coefficients
*              in [-(GAMMA1 - 1), GAMMA1].
*
* Arguments:   - ml_dsa_params: parameter struct
*              - poly *r: pointer to output polynomial
*              - const uint8_t *a: byte array with bit-packed polynomial
**************************************************/
void polyz_unpack(ml_dsa_params *params, poly *r, const uint8_t *a) {
  unsigned int i;

  assert((params->gamma1 == (1 << 17)) ||
     (params->gamma1 == (1 << 19)));

  if (params->gamma1 == (1 << 17)) {
    for(i = 0; i < N/4; ++i) {
      r->coeffs[4*i+0]  = a[9*i+0];
      r->coeffs[4*i+0] |= (uint32_t)a[9*i+1] << 8;
      r->coeffs[4*i+0] |= (uint32_t)a[9*i+2] << 16;
      r->coeffs[4*i+0] &= 0x3FFFF;

      r->coeffs[4*i+1]  = a[9*i+2] >> 2;
      r->coeffs[4*i+1] |= (uint32_t)a[9*i+3] << 6;
      r->coeffs[4*i+1] |= (uint32_t)a[9*i+4] << 14;
      r->coeffs[4*i+1] &= 0x3FFFF;

      r->coeffs[4*i+2]  = a[9*i+4] >> 4;
      r->coeffs[4*i+2] |= (uint32_t)a[9*i+5] << 4;
      r->coeffs[4*i+2] |= (uint32_t)a[9*i+6] << 12;
      r->coeffs[4*i+2] &= 0x3FFFF;

      r->coeffs[4*i+3]  = a[9*i+6] >> 6;
      r->coeffs[4*i+3] |= (uint32_t)a[9*i+7] << 2;
      r->coeffs[4*i+3] |= (uint32_t)a[9*i+8] << 10;
      r->coeffs[4*i+3] &= 0x3FFFF;

      r->coeffs[4*i+0] = params->gamma1 - r->coeffs[4*i+0];
      r->coeffs[4*i+1] = params->gamma1 - r->coeffs[4*i+1];
      r->coeffs[4*i+2] = params->gamma1 - r->coeffs[4*i+2];
      r->coeffs[4*i+3] = params->gamma1 - r->coeffs[4*i+3];
    }
  }
  else if (params->gamma1 == (1 << 19)) {
    for(i = 0; i < N/2; ++i) {
      r->coeffs[2*i+0]  = a[5*i+0];
      r->coeffs[2*i+0] |= (uint32_t)a[5*i+1] << 8;
      r->coeffs[2*i+0] |= (uint32_t)a[5*i+2] << 16;
      r->coeffs[2*i+0] &= 0xFFFFF;

      r->coeffs[2*i+1]  = a[5*i+2] >> 4;
      r->coeffs[2*i+1] |= (uint32_t)a[5*i+3] << 4;
      r->coeffs[2*i+1] |= (uint32_t)a[5*i+4] << 12;
      /* r->coeffs[2*i+1] &= 0xFFFFF; */ /* No effect, since we're anyway at 20 bits */

      r->coeffs[2*i+0] = params->gamma1 - r->coeffs[2*i+0];
      r->coeffs[2*i+1] = params->gamma1 - r->coeffs[2*i+1];
    }
  }
}

/*************************************************
* Name:        polyw1_pack
*
* Description: Bit-pack polynomial w1 with coefficients in [0,15] or [0,43].
*              Input coefficients are assumed to be standard representatives.
*
* Arguments:   - ml_dsa_params: parameter struct
*              - uint8_t *r: pointer to output byte array with at least
*                            POLYW1_PACKEDBYTES bytes
*              - const poly *a: pointer to input polynomial
**************************************************/
void polyw1_pack(ml_dsa_params *params, uint8_t *r, const poly *a) {
  unsigned int i;

  if (params->gamma2 == (Q-1)/88) {
    for(i = 0; i < N/4; ++i) {
      r[3*i+0]  = a->coeffs[4*i+0];
      r[3*i+0] |= a->coeffs[4*i+1] << 6;
      r[3*i+1]  = a->coeffs[4*i+1] >> 2;
      r[3*i+1] |= a->coeffs[4*i+2] << 4;
      r[3*i+2]  = a->coeffs[4*i+2] >> 4;
      r[3*i+2] |= a->coeffs[4*i+3] << 2;
    }
  }
  else if (params->gamma2 == (Q-1)/32) {
    for(i = 0; i < N/2; ++i)
      r[i] = a->coeffs[2*i+0] | (a->coeffs[2*i+1] << 4);
  }
}
