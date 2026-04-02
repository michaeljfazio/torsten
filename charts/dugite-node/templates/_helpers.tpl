{{/*
Expand the name of the chart.
*/}}
{{- define "dugite-node.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "dugite-node.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "dugite-node.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "dugite-node.labels" -}}
helm.sh/chart: {{ include "dugite-node.chart" . }}
{{ include "dugite-node.selectorLabels" . }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/component: {{ .Values.role }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "dugite-node.selectorLabels" -}}
app.kubernetes.io/name: {{ include "dugite-node.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "dugite-node.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "dugite-node.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Network magic value
*/}}
{{- define "dugite-node.networkMagic" -}}
{{- if .Values.network.magic }}
{{- .Values.network.magic }}
{{- else if eq .Values.network.name "mainnet" }}
{{- 764824073 }}
{{- else if eq .Values.network.name "preview" }}
{{- 2 }}
{{- else if eq .Values.network.name "preprod" }}
{{- 1 }}
{{- else }}
{{- 2 }}
{{- end }}
{{- end }}

{{/*
Config file name for network
*/}}
{{- define "dugite-node.configFile" -}}
{{- printf "%s-config.json" .Values.network.name }}
{{- end }}

{{/*
Topology file name for network
*/}}
{{- define "dugite-node.topologyFile" -}}
{{- printf "%s-topology.json" .Values.network.name }}
{{- end }}
