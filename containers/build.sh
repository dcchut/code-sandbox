#!/bin/bash
repository=dcchut

perform_push="${PERFORM_PUSH-false}"

# Build our images
docker build --build-arg channel=stable -t dcchut/code-sandbox-rust-stable ./rust
docker build -t dcchut/code-sandbox-python ./python

docker tag dcchut/code-sandbox-rust-stable code-sandbox-rust-stable
docker tag dcchut/code-sandbox-python code-sandbox-python

if [[ "${perform_push}" == 'true' ]]; then
    docker push dcchut/code-sandbox-rust-stable
    docker push dcchut/code-sandbox-python
fi