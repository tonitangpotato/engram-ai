#!/bin/bash
# Engram Pensieve — Start backend + frontend
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"

echo "✨ Starting Engram Pensieve..."
echo ""

# Backend
echo "🔮 Starting backend (port 8877)..."
cd "$DIR/backend"
pip install -q -r requirements.txt 2>/dev/null
python3 app.py &
BACKEND_PID=$!

# Frontend
echo "🌙 Starting frontend (port 5173)..."
cd "$DIR/frontend"
npm run dev &
FRONTEND_PID=$!

echo ""
echo "✨ Engram Pensieve is running!"
echo "   Frontend: http://localhost:5173"
echo "   Backend:  http://localhost:8877"
echo ""
echo "Press Ctrl+C to stop."

trap "kill $BACKEND_PID $FRONTEND_PID 2>/dev/null" EXIT
wait
