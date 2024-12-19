// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC
// Wed Dec 11 13:50:30 UTC 2024

use crate::cc_builder::Library;

pub(super) const CRYPTO_LIBRARY: Library = Library {
    name: "crypto",
    // This attribute is intentionally let blank
    flags: &[],
    sources: &[
        "crypto/asn1/a_bitstr.c",
        "crypto/asn1/a_bool.c",
        "crypto/asn1/a_d2i_fp.c",
        "crypto/asn1/a_dup.c",
        "crypto/asn1/a_gentm.c",
        "crypto/asn1/a_i2d_fp.c",
        "crypto/asn1/a_int.c",
        "crypto/asn1/a_mbstr.c",
        "crypto/asn1/a_object.c",
        "crypto/asn1/a_octet.c",
        "crypto/asn1/a_strex.c",
        "crypto/asn1/a_strnid.c",
        "crypto/asn1/a_time.c",
        "crypto/asn1/a_type.c",
        "crypto/asn1/a_utctm.c",
        "crypto/asn1/a_utf8.c",
        "crypto/asn1/asn1_lib.c",
        "crypto/asn1/asn1_par.c",
        "crypto/asn1/asn_pack.c",
        "crypto/asn1/f_int.c",
        "crypto/asn1/f_string.c",
        "crypto/asn1/posix_time.c",
        "crypto/asn1/tasn_dec.c",
        "crypto/asn1/tasn_enc.c",
        "crypto/asn1/tasn_fre.c",
        "crypto/asn1/tasn_new.c",
        "crypto/asn1/tasn_typ.c",
        "crypto/asn1/tasn_utl.c",
        "crypto/base64/base64.c",
        "crypto/bio/bio.c",
        "crypto/bio/bio_mem.c",
        "crypto/bio/connect.c",
        "crypto/bio/errno.c",
        "crypto/bio/fd.c",
        "crypto/bio/file.c",
        "crypto/bio/hexdump.c",
        "crypto/bio/pair.c",
        "crypto/bio/printf.c",
        "crypto/bio/socket.c",
        "crypto/bio/socket_helper.c",
        "crypto/blake2/blake2.c",
        "crypto/bn_extra/bn_asn1.c",
        "crypto/bn_extra/convert.c",
        "crypto/buf/buf.c",
        "crypto/bytestring/asn1_compat.c",
        "crypto/bytestring/ber.c",
        "crypto/bytestring/cbb.c",
        "crypto/bytestring/cbs.c",
        "crypto/bytestring/unicode.c",
        "crypto/chacha/chacha.c",
        "crypto/cipher_extra/cipher_extra.c",
        "crypto/cipher_extra/derive_key.c",
        "crypto/cipher_extra/e_aes_cbc_hmac_sha1.c",
        "crypto/cipher_extra/e_aes_cbc_hmac_sha256.c",
        "crypto/cipher_extra/e_aesctrhmac.c",
        "crypto/cipher_extra/e_aesgcmsiv.c",
        "crypto/cipher_extra/e_chacha20poly1305.c",
        "crypto/cipher_extra/e_des.c",
        "crypto/cipher_extra/e_null.c",
        "crypto/cipher_extra/e_rc2.c",
        "crypto/cipher_extra/e_rc4.c",
        "crypto/cipher_extra/e_tls.c",
        "crypto/cipher_extra/tls_cbc.c",
        "crypto/conf/conf.c",
        "crypto/crypto.c",
        "crypto/decrepit/bio/base64_bio.c",
        "crypto/decrepit/blowfish/blowfish.c",
        "crypto/decrepit/cast/cast.c",
        "crypto/decrepit/cast/cast_tables.c",
        "crypto/decrepit/cfb/cfb.c",
        "crypto/decrepit/dh/dh_decrepit.c",
        "crypto/decrepit/evp/evp_do_all.c",
        "crypto/decrepit/obj/obj_decrepit.c",
        "crypto/decrepit/ripemd/ripemd.c",
        "crypto/decrepit/rsa/rsa_decrepit.c",
        "crypto/decrepit/x509/x509_decrepit.c",
        "crypto/des/des.c",
        "crypto/dh_extra/dh_asn1.c",
        "crypto/dh_extra/params.c",
        "crypto/digest_extra/digest_extra.c",
        "crypto/dsa/dsa.c",
        "crypto/dsa/dsa_asn1.c",
        "crypto/ec_extra/ec_asn1.c",
        "crypto/ec_extra/ec_derive.c",
        "crypto/ec_extra/hash_to_curve.c",
        "crypto/ecdh_extra/ecdh_extra.c",
        "crypto/ecdsa_extra/ecdsa_asn1.c",
        "crypto/engine/engine.c",
        "crypto/err/err.c",
        "crypto/evp_extra/evp_asn1.c",
        "crypto/evp_extra/p_dh.c",
        "crypto/evp_extra/p_dh_asn1.c",
        "crypto/evp_extra/p_dsa.c",
        "crypto/evp_extra/p_dsa_asn1.c",
        "crypto/evp_extra/p_ec_asn1.c",
        "crypto/evp_extra/p_ed25519_asn1.c",
        "crypto/evp_extra/p_hmac_asn1.c",
        "crypto/evp_extra/p_kem_asn1.c",
        "crypto/evp_extra/p_methods.c",
        "crypto/evp_extra/p_rsa_asn1.c",
        "crypto/evp_extra/p_x25519.c",
        "crypto/evp_extra/p_x25519_asn1.c",
        "crypto/evp_extra/print.c",
        "crypto/evp_extra/scrypt.c",
        "crypto/evp_extra/sign.c",
        "crypto/ex_data.c",
        "crypto/fipsmodule/bcm.c",
        "crypto/fipsmodule/cpucap/cpucap.c",
        "crypto/fipsmodule/fips_shared_support.c",
        "crypto/hpke/hpke.c",
        "crypto/hrss/hrss.c",
        "crypto/kyber/kem_kyber.c",
        "crypto/kyber/kyber1024r3_ref.c",
        "crypto/kyber/kyber512r3_ref.c",
        "crypto/kyber/kyber768r3_ref.c",
        "crypto/kyber/pqcrystals_kyber_ref_common/fips202.c",
        "crypto/lhash/lhash.c",
        "crypto/mem.c",
        "crypto/obj/obj.c",
        "crypto/obj/obj_xref.c",
        "crypto/ocsp/ocsp_asn.c",
        "crypto/ocsp/ocsp_client.c",
        "crypto/ocsp/ocsp_extension.c",
        "crypto/ocsp/ocsp_http.c",
        "crypto/ocsp/ocsp_lib.c",
        "crypto/ocsp/ocsp_print.c",
        "crypto/ocsp/ocsp_server.c",
        "crypto/ocsp/ocsp_verify.c",
        "crypto/pem/pem_all.c",
        "crypto/pem/pem_info.c",
        "crypto/pem/pem_lib.c",
        "crypto/pem/pem_oth.c",
        "crypto/pem/pem_pk8.c",
        "crypto/pem/pem_pkey.c",
        "crypto/pem/pem_x509.c",
        "crypto/pem/pem_xaux.c",
        "crypto/pkcs7/bio/cipher.c",
        "crypto/pkcs7/bio/md.c",
        "crypto/pkcs7/pkcs7.c",
        "crypto/pkcs7/pkcs7_asn1.c",
        "crypto/pkcs7/pkcs7_x509.c",
        "crypto/pkcs8/p5_pbev2.c",
        "crypto/pkcs8/pkcs8.c",
        "crypto/pkcs8/pkcs8_x509.c",
        "crypto/poly1305/poly1305.c",
        "crypto/poly1305/poly1305_arm.c",
        "crypto/poly1305/poly1305_vec.c",
        "crypto/pool/pool.c",
        "crypto/rand_extra/deterministic.c",
        "crypto/rand_extra/entropy_passive.c",
        "crypto/rand_extra/forkunsafe.c",
        "crypto/rand_extra/fuchsia.c",
        "crypto/rand_extra/rand_extra.c",
        "crypto/rand_extra/trusty.c",
        "crypto/rand_extra/windows.c",
        "crypto/rc4/rc4.c",
        "crypto/refcount_c11.c",
        "crypto/refcount_lock.c",
        "crypto/refcount_win.c",
        "crypto/rsa_extra/rsa_asn1.c",
        "crypto/rsa_extra/rsa_crypt.c",
        "crypto/rsa_extra/rsa_print.c",
        "crypto/rsa_extra/rsassa_pss_asn1.c",
        "crypto/siphash/siphash.c",
        "crypto/spake25519/spake25519.c",
        "crypto/stack/stack.c",
        "crypto/thread.c",
        "crypto/thread_none.c",
        "crypto/thread_pthread.c",
        "crypto/thread_win.c",
        "crypto/trust_token/pmbtoken.c",
        "crypto/trust_token/trust_token.c",
        "crypto/trust_token/voprf.c",
        "crypto/x509/a_digest.c",
        "crypto/x509/a_sign.c",
        "crypto/x509/a_verify.c",
        "crypto/x509/algorithm.c",
        "crypto/x509/asn1_gen.c",
        "crypto/x509/by_dir.c",
        "crypto/x509/by_file.c",
        "crypto/x509/i2d_pr.c",
        "crypto/x509/name_print.c",
        "crypto/x509/policy.c",
        "crypto/x509/rsa_pss.c",
        "crypto/x509/t_crl.c",
        "crypto/x509/t_req.c",
        "crypto/x509/t_x509.c",
        "crypto/x509/t_x509a.c",
        "crypto/x509/v3_akey.c",
        "crypto/x509/v3_akeya.c",
        "crypto/x509/v3_alt.c",
        "crypto/x509/v3_bcons.c",
        "crypto/x509/v3_bitst.c",
        "crypto/x509/v3_conf.c",
        "crypto/x509/v3_cpols.c",
        "crypto/x509/v3_crld.c",
        "crypto/x509/v3_enum.c",
        "crypto/x509/v3_extku.c",
        "crypto/x509/v3_genn.c",
        "crypto/x509/v3_ia5.c",
        "crypto/x509/v3_info.c",
        "crypto/x509/v3_int.c",
        "crypto/x509/v3_lib.c",
        "crypto/x509/v3_ncons.c",
        "crypto/x509/v3_ocsp.c",
        "crypto/x509/v3_pcons.c",
        "crypto/x509/v3_pmaps.c",
        "crypto/x509/v3_prn.c",
        "crypto/x509/v3_purp.c",
        "crypto/x509/v3_skey.c",
        "crypto/x509/v3_utl.c",
        "crypto/x509/x509.c",
        "crypto/x509/x509_att.c",
        "crypto/x509/x509_cmp.c",
        "crypto/x509/x509_d2.c",
        "crypto/x509/x509_def.c",
        "crypto/x509/x509_ext.c",
        "crypto/x509/x509_lu.c",
        "crypto/x509/x509_obj.c",
        "crypto/x509/x509_req.c",
        "crypto/x509/x509_set.c",
        "crypto/x509/x509_trs.c",
        "crypto/x509/x509_txt.c",
        "crypto/x509/x509_v3.c",
        "crypto/x509/x509_vfy.c",
        "crypto/x509/x509_vpm.c",
        "crypto/x509/x509cset.c",
        "crypto/x509/x509name.c",
        "crypto/x509/x509rset.c",
        "crypto/x509/x509spki.c",
        "crypto/x509/x_algor.c",
        "crypto/x509/x_all.c",
        "crypto/x509/x_attrib.c",
        "crypto/x509/x_crl.c",
        "crypto/x509/x_exten.c",
        "crypto/x509/x_name.c",
        "crypto/x509/x_pubkey.c",
        "crypto/x509/x_req.c",
        "crypto/x509/x_sig.c",
        "crypto/x509/x_spki.c",
        "crypto/x509/x_val.c",
        "crypto/x509/x_x509.c",
        "crypto/x509/x_x509a.c",
        "generated-src/err_data.c",
        "generated-src/linux-aarch64/crypto/chacha/chacha-armv8.S",
        "generated-src/linux-aarch64/crypto/cipher_extra/chacha20_poly1305_armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/aesv8-armx.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/aesv8-gcm-armv8-unroll8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/aesv8-gcm-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/armv8-mont.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/bn-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/ghash-neon-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/ghashv8-armx.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/keccak1600-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/md5-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/p256-armv8-asm.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/p256_beeu-armv8-asm.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/sha1-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/sha256-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/sha512-armv8.S",
        "generated-src/linux-aarch64/crypto/fipsmodule/vpaes-armv8.S",
        "generated-src/linux-aarch64/crypto/test/trampoline-armv8.S",
        "third_party/s2n-bignum/arm/curve25519/bignum_madd_n25519.S",
        "third_party/s2n-bignum/arm/curve25519/bignum_madd_n25519_alt.S",
        "third_party/s2n-bignum/arm/curve25519/bignum_mod_n25519.S",
        "third_party/s2n-bignum/arm/curve25519/bignum_neg_p25519.S",
        "third_party/s2n-bignum/arm/curve25519/curve25519_x25519_byte.S",
        "third_party/s2n-bignum/arm/curve25519/curve25519_x25519_byte_alt.S",
        "third_party/s2n-bignum/arm/curve25519/curve25519_x25519base_byte.S",
        "third_party/s2n-bignum/arm/curve25519/curve25519_x25519base_byte_alt.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_decode.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_decode_alt.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_encode.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_scalarmulbase.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_scalarmulbase_alt.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_scalarmuldouble.S",
        "third_party/s2n-bignum/arm/curve25519/edwards25519_scalarmuldouble_alt.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_emontredc_8n.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_emontredc_8n_neon.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_kmul_16_32.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_kmul_16_32_neon.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_kmul_32_64.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_kmul_32_64_neon.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_ksqr_16_32.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_ksqr_16_32_neon.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_ksqr_32_64.S",
        "third_party/s2n-bignum/arm/fastmul/bignum_ksqr_32_64_neon.S",
        "third_party/s2n-bignum/arm/generic/bignum_copy_row_from_table.S",
        "third_party/s2n-bignum/arm/generic/bignum_copy_row_from_table_16_neon.S",
        "third_party/s2n-bignum/arm/generic/bignum_copy_row_from_table_32_neon.S",
        "third_party/s2n-bignum/arm/generic/bignum_copy_row_from_table_8n_neon.S",
        "third_party/s2n-bignum/arm/generic/bignum_ge.S",
        "third_party/s2n-bignum/arm/generic/bignum_mul.S",
        "third_party/s2n-bignum/arm/generic/bignum_optsub.S",
        "third_party/s2n-bignum/arm/generic/bignum_sqr.S",
        "third_party/s2n-bignum/arm/p384/bignum_add_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_deamont_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_littleendian_6.S",
        "third_party/s2n-bignum/arm/p384/bignum_montmul_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_montmul_p384_alt.S",
        "third_party/s2n-bignum/arm/p384/bignum_montsqr_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_montsqr_p384_alt.S",
        "third_party/s2n-bignum/arm/p384/bignum_neg_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_nonzero_6.S",
        "third_party/s2n-bignum/arm/p384/bignum_sub_p384.S",
        "third_party/s2n-bignum/arm/p384/bignum_tomont_p384.S",
        "third_party/s2n-bignum/arm/p384/p384_montjdouble.S",
        "third_party/s2n-bignum/arm/p384/p384_montjdouble_alt.S",
        "third_party/s2n-bignum/arm/p521/bignum_add_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_fromlebytes_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_mul_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_mul_p521_alt.S",
        "third_party/s2n-bignum/arm/p521/bignum_neg_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_sqr_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_sqr_p521_alt.S",
        "third_party/s2n-bignum/arm/p521/bignum_sub_p521.S",
        "third_party/s2n-bignum/arm/p521/bignum_tolebytes_p521.S",
        "third_party/s2n-bignum/arm/p521/p521_jdouble.S",
        "third_party/s2n-bignum/arm/p521/p521_jdouble_alt.S",
    ],
};
