FROM golang:1.27-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY *.go ./
# idemixgen at the IBM/idemix version linked into Fabric 2.5.16 (go.mod: tool directive), for
# the Idemix organisation's issuer key and member credential (generate.sh with IDEMIX=1).
RUN CGO_ENABLED=0 go build -trimpath -o /loadgen . && \
    CGO_ENABLED=0 go build -trimpath -o /idemixgen github.com/IBM/idemix/tools/idemixgen

FROM alpine:3.20
COPY --from=build /loadgen /idemixgen /
ENTRYPOINT ["/loadgen"]
