FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY worker.py .

ENV TEMPORAL_ADDRESS=temporal:7233 \
    TEMPORAL_NAMESPACE=default \
    TEMPORAL_TASK_QUEUE=mcp-js \
    MCP_SERVER_URL=http://node1:3000 \
    HTTP_PORT=8000

EXPOSE 8000

CMD ["python", "worker.py"]
