This is a midenVM verifier implementation.
File helpers.rs is inspired/copied from core midenVM cli verifier implementation since those methods are not exposed by midenVM crate.


In order to verify midenVM proofs you'd need program_hash, proof_file path, stack_input path and stack_output path.

Simple example of a program is placed in /example folder. Simple example of verifying this program could be done this way:
```
target/debug/midenvm-verifier 78d31702eb946e1817e3c0881fcb3739562f08fbddf202de2eae241b9968ab3b ./midenvm-verifier/example/fib.proof ./midenvm-verifier/example/fib.inputs ./midenvm-verifier/example/fib.outputs
```

In order to get these files you'd need to install miden-vm and generate them based on your program.masm file.

Check https://0xpolygonmiden.github.io/miden-vm/intro/usage.html#running-miden-vm for detailed instructions.

Once you have your program.masm file run:
```
./target/optimized/miden prove /path-to-your-program/program.masm
```

And you will get your `program_hash` and `proof_file, stack_input path and stack_output` files.

