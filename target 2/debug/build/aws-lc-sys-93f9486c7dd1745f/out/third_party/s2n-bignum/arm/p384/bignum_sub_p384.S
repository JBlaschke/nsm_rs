# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_sub_p384.S"
# 1 "<built-in>" 1
# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_sub_p384.S" 2
# 13 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_sub_p384.S"
# 1 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/include/_internal_s2n_bignum.h" 1
# 14 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_sub_p384.S" 2

        .globl _bignum_sub_p384

        .text
        .balign 4
# 34 "/Users/sofiamorris/Desktop/nsm_rs/vendor/aws-lc-sys/aws-lc/third_party/s2n-bignum/arm/p384/bignum_sub_p384.S"
_bignum_sub_p384:




        ldp x5, x6, [x1]
        ldp x4, x3, [x2]
        subs x5, x5, x4
        sbcs x6, x6, x3
        ldp x7, x8, [x1, #16]
        ldp x4, x3, [x2, #16]
        sbcs x7, x7, x4
        sbcs x8, x8, x3
        ldp x9, x10, [x1, #32]
        ldp x4, x3, [x2, #32]
        sbcs x9, x9, x4
        sbcs x10, x10, x3



        csetm x3, cc



        mov x4, #0x00000000ffffffff
        and x4, x4, x3
        adds x5, x5, x4
        eor x4, x4, x3
        adcs x6, x6, x4
        mov x4, #0xfffffffffffffffe
        and x4, x4, x3
        adcs x7, x7, x4
        adcs x8, x8, x3
        adcs x9, x9, x3
        adc x10, x10, x3



        stp x5, x6, [x0]
        stp x7, x8, [x0, #16]
        stp x9, x10, [x0, #32]

        ret
