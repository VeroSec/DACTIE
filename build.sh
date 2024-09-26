#!/bin/bash

#Generates Key Material
mkdir ./key_material/
echo "BOOT_PEER_ID=<insert_here>" > ./key_material/boot_id.env
for i in {0..9}; do mkdir -p ./key_material/key_material_$i; done
#Builds Docker images
docker build -t dactie-peer -f ./dactie-peer/Dockerfile .
docker build -t dactie-authority -f ./dactie-authority/Dockerfile .
docker build -t dactie-archive -f ./dactie-archive/Dockerfile .

