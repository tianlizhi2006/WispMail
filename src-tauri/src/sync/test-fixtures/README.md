# Local TLS test fixtures

These certificates and the deliberately public private key belong only to the
loopback test mail server. They are not production credentials. The issuing root
is trusted only by test-specific client builders; it is never installed into an
operating-system trust store or included in a production connector.

`server.pem` covers `localhost` and `127.0.0.1`, with validity from 2020 to 2120.
`server-key.pem` is its unencrypted PKCS#8 RSA key. `root.pem` is the test issuer;
the issuer private key is not retained. The fixture allows tests to exercise real
TLS and STARTTLS, authentication success/rejection, and rejection by the default
system trust configuration without a network connection to an external service.
