#!/usr/bin/env bash

awslocal s3api create-bucket --bucket golem-compilation-cache
awslocal s3api create-bucket --bucket golem-custom-data
awslocal s3api create-bucket --bucket golem-oplog-payload
awslocal s3api create-bucket --bucket golem-oplog-archive-1
awslocal s3api create-bucket --bucket golem-initial-component-files

# signal setup is done
awslocal s3api create-bucket --bucket signal-ready
