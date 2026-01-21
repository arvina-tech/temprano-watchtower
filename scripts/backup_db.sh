#!/bin/bash

# To run this script daily at 1 AM, add the following to your crontab (crontab -e):
# HOME=/home/backup-manager
# 0 1 * * * $HOME/db-backup/backup_db.sh >> $HOME/db-backup/backup.log 2>&1
#
# This script needs the following packages:
# - awscli
# - postgresql-client
# - gpg
#
# This script needs the following environment variables:
# - DB_HOST
# - DB_PORT
# - DB_NAME
# - DB_USER
# - DB_PASSWORD
# - S3_BUCKET
# - AWS_ACCESS_KEY_ID
# - AWS_SECRET_ACCESS_KEY
# - AWS_REGION

# Exit on any error
set -e

# Get the directory where the script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# Load environment variables from .env file
if [ -f "${SCRIPT_DIR}/.env" ]; then
    set -a
    source "${SCRIPT_DIR}/.env"
    set +a
fi

# Configuration
BACKUP_DIR="$HOME/db-backup/backups"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/backup_${TIMESTAMP}.dump"
ENCRYPTED_FILE="${BACKUP_FILE}.gpg"
S3_BUCKET="tempo-watchtower-backups"
GPG_RECIPIENT="security@temprano.io"

# Create backup directory if it doesn't exist
mkdir -p "${BACKUP_DIR}"

# Function to clean up temporary files
cleanup() {
    rm -f "${BACKUP_FILE}" "${ENCRYPTED_FILE}"
}

# Register cleanup function to run on script exit
trap cleanup EXIT

echo "Starting database backup..."

# Dump the database
PGPASSWORD="${DB_PASSWORD}" pg_dump \
    -h "${DB_HOST}" \
    -p "${DB_PORT}" \
    -U "${DB_USER}" \
    -d "${DB_NAME}" \
    --format=custom \
    --compress=9 \
    --blobs \
    --verbose \
    --data-only \
    --exclude-table-data='public._sqlx_migrations' \
    > "${BACKUP_FILE}"

if [ $? -ne 0 ]; then
    echo "Error: Database dump failed"
    exit 1
fi

echo "Encrypting backup..."
# Encrypt the backup
gpg --encrypt --recipient "${GPG_RECIPIENT}" --output "${ENCRYPTED_FILE}" "${BACKUP_FILE}"
if [ $? -ne 0 ]; then
    echo "Error: Encryption failed"
    exit 1
fi

echo "Uploading to R2..."
# Upload to R2
aws s3 cp "${ENCRYPTED_FILE}" "s3://${S3_BUCKET}/db-backups/backup.dump.gpg"
if [ $? -ne 0 ]; then
    echo "Error: R2 upload failed"
    exit 1
fi

echo "Backup completed successfully!"
