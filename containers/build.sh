#!/bin/bash

repository=dcchut

base_image_name = "code-sandbox-"

# Build rust image
image_name="${base_image_name}rust-stable"
full_name="${repository}/${image_name}"