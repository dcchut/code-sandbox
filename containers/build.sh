#!/bin/bash
repository=dcchut

perform_push="${PERFORM_PUSH-false}"

# Build our images
docker build --build-arg channel=stable -t dcchut/code-sandbox-rust-stable ./rust
docker build -t dcchut/code-sandbox-python ./python
docker build -t dcchut/code-sandbox-haskell ./haskell

docker tag dcchut/code-sandbox-rust-stable code-sandbox-rust-stable
docker tag dcchut/code-sandbox-python code-sandbox-python
docker tag dcchut/code-sandbox-haskell code-sandbox-haskell


if [[ "${perform_push}" == 'true' ]]; then
    docker push dcchut/code-sandbox-rust-stable
    docker push dcchut/code-sandbox-python
    docker push dcchut/code-sandbox-haskell
fi