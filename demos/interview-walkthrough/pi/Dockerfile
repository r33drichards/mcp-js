FROM node@sha256:55fb5f5a527ea2c5c4a95f963e48ef90aa38c7b346054fdc8ee9957a2b55d205
WORKDIR /demo
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts --no-audit --no-fund
COPY paired-fork.ts ./
ENV PI_CODING_AGENT_DIR=/state/agent
# No model call on startup. Explicit profile + interactive run needed for prompting.
ENTRYPOINT ["./node_modules/.bin/pi", "--session-dir", "/state/sessions", "--tools", "codemode", "--extension", "/demo/paired-fork.ts"]
CMD ["--help"]
