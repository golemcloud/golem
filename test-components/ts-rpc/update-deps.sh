#!/bin/sh

pushd caller
wit-deps update
popd
pushd counter
wit-deps update
popd
pushd counter-stub
wit-deps update
popd
