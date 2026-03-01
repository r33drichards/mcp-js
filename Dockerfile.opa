FROM alpine:3.19
RUN wget -O /usr/local/bin/opa https://github.com/open-policy-agent/opa/releases/latest/download/opa_linux_amd64_static \
    && chmod +x /usr/local/bin/opa
COPY policies /policies
EXPOSE 8181
ENTRYPOINT ["/usr/local/bin/opa"]
CMD ["run", "--server", "--addr", "0.0.0.0:8181", "/policies"]
