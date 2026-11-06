Test fixtures only. `test_rsa.pem` is a throwaway RSA key generated for the ID-token unit
tests (`id_token.rs`); it signs nothing outside tests and is not a secret. `test_rsa_n.txt` is
its modulus, base64url-encoded.
