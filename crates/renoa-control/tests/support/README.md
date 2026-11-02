# Test support files

`push-test-key.pem` is a 2048-bit RSA private key generated with OpenSSL for
the push tests only. It belongs to no Google service account, signs nothing
outside these tests, and grants access to nothing. A secret scanner that flags
it has found this test key, not a leaked credential.
