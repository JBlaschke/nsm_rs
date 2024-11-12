# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_nonzero_6.S"
# 1 "<built-in>" 1
# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_nonzero_6.S" 2
# 12 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_nonzero_6.S"
# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/include/_internal_s2n_bignum.h" 1
# 13 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_nonzero_6.S" 2

        .globl _bignum_nonzero_6

        .text
        .balign 4







_bignum_nonzero_6:



        ldp x1, x2, [x0]
        orr x1, x1, x2
        ldp x3, x2, [x0, #16]
        orr x3, x3, x2
        orr x1, x1, x3
        ldp x3, x2, [x0, #32]
        orr x3, x3, x2
        orr x1, x1, x3



        cmp x1, xzr
        cset x0, ne

        ret
