SELECT 'CREATE DATABASE claude_code_proxy_test OWNER claude_code_proxy'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'claude_code_proxy_test')
\gexec
