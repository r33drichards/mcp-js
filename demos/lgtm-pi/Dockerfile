FROM grafana/mcp-grafana@sha256:71a3f4e6f412d253611314233df709ec2571fb50e39742d23fa2940f670646dc AS grafana_mcp
FROM mcp-lgtm-engine:fe5b613 AS engine
FROM mcp-lgtm-browser:63bab526
COPY --from=engine /server /server
COPY --from=engine /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
COPY --from=engine /usr /usr
COPY --from=engine /lib /lib
COPY --from=engine /lib64 /lib64
COPY --from=grafana_mcp /app/mcp-grafana /minimal/mcp-grafana
COPY *.rego *-policies.json mcp.json supervisor.py /minimal/
ENTRYPOINT ["/usr/bin/python3", "/minimal/supervisor.py"]
