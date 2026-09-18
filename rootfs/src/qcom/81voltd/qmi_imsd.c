/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Copyright (c) 2024, Richard Acayan. All rights reserved. */
#include <errno.h>
#include <string.h>
#include "qmi_imsd.h"

struct qmi_elem_info imsd_wds_spec_ei[] = {
	{
		.data_type = QMI_STRING,
		.elem_len = 256,
		.elem_size = sizeof(char),
		.offset = offsetof(struct imsd_wds_spec, apn)
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.offset = offsetof(struct imsd_wds_spec, unkfield_0_5),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.offset = offsetof(struct imsd_wds_spec, profiles_select),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.offset = offsetof(struct imsd_wds_spec, ip_family),
	},
	{
		.data_type = QMI_UNSIGNED_2_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint16_t),
		.offset = offsetof(struct imsd_wds_spec, profile_idx_3gpp),
	},
	{
		.data_type = QMI_UNSIGNED_2_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint16_t),
		.offset = offsetof(struct imsd_wds_spec, profile_idx_3gpp2),
	},
	{}
};

struct qmi_elem_info imsd_address_ei[] = {
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.offset = offsetof(struct imsd_address, family),
	},
	{
		.data_type = QMI_STRING,
		.elem_len = 256,
		.elem_size = sizeof(char),
		.offset = offsetof(struct imsd_address, addr)
	},
	{}
};

struct qmi_elem_info imsd_op_res_ei[] = {
	{
		.data_type = QMI_UNSIGNED_2_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint16_t),
		.offset = offsetof(struct imsd_op_res, err_status),
	},
	{
		.data_type = QMI_UNSIGNED_2_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint16_t),
		.offset = offsetof(struct imsd_op_res, err_code),
	},
	{}
};

struct qmi_elem_info imsd_start_connection_req_ei[] = {
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_wds_spec),
		.tlv_type = 1,
		.offset = offsetof(struct imsd_start_connection_req, conn_params),
		.ei_array = imsd_wds_spec_ei,
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 16,
		.offset = offsetof(struct imsd_start_connection_req, connection),
	},
	{
		.data_type = QMI_OPT_FLAG,
		.elem_len = 1,
		.elem_size = sizeof(bool),
		.tlv_type = 18,
		.offset = offsetof(struct imsd_start_connection_req, echo_sub_valid),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 18,
		.offset = offsetof(struct imsd_start_connection_req, echo_sub),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 19,
		.offset = offsetof(struct imsd_start_connection_req, subscription),
	},
	{}
};

struct qmi_elem_info imsd_start_connection_resp_ei[] = {
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_op_res),
		.tlv_type = 2,
		.offset = offsetof(struct imsd_start_connection_resp, res),
		.ei_array = imsd_op_res_ei,
	},
	{
		.data_type = QMI_UNSIGNED_1_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint8_t),
		.tlv_type = 16,
		.offset = offsetof(struct imsd_start_connection_resp, connection),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 17,
		.offset = offsetof(struct imsd_start_connection_resp, orig_conn_id),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 18,
		.offset = offsetof(struct imsd_start_connection_resp, subscription),
	},
	{}
};

struct qmi_elem_info imsd_connection_changed_ind_ei[] = {
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_op_res),
		.tlv_type = 2,
		.offset = offsetof(struct imsd_connection_changed_ind, res),
		.ei_array = imsd_op_res_ei,
	},
	{
		.data_type = QMI_UNSIGNED_1_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint8_t),
		.tlv_type = 1,
		.offset = offsetof(struct imsd_connection_changed_ind, connection),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 16,
		.offset = offsetof(struct imsd_connection_changed_ind, orig_conn_id),
	},
	{
		.data_type = QMI_OPT_FLAG,
		.elem_len = 1,
		.elem_size = sizeof(bool),
		.tlv_type = 17,
		.offset = offsetof(struct imsd_connection_changed_ind, ip_addr_valid),
	},
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_address),
		.tlv_type = 17,
		.offset = offsetof(struct imsd_connection_changed_ind, ip_addr),
		.ei_array = imsd_address_ei,
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 18,
		.offset = offsetof(struct imsd_connection_changed_ind, subscription),
	},
	{}
};

struct qmi_elem_info imsd_stop_connection_req_ei[] = {
	{
		.data_type = QMI_UNSIGNED_1_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint8_t),
		.tlv_type = 1,
		.offset = offsetof(struct imsd_stop_connection_req, connection),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 16,
		.offset = offsetof(struct imsd_stop_connection_req, unkecho_01),
	},
	{}
};

struct qmi_elem_info imsd_stop_connection_resp_ei[] = {
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_op_res),
		.tlv_type = 2,
		.offset = offsetof(struct imsd_stop_connection_resp, res),
		.ei_array = imsd_op_res_ei,
	},
	{
		.data_type = QMI_UNSIGNED_1_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint8_t),
		.tlv_type = 16,
		.offset = offsetof(struct imsd_stop_connection_resp, unkfield_FA),
	},
	{
		.data_type = QMI_UNSIGNED_4_BYTE,
		.elem_len = 1,
		.elem_size = sizeof(uint32_t),
		.tlv_type = 17,
		.offset = offsetof(struct imsd_stop_connection_resp, unkecho_01),
	},
	{}
};

struct qmi_elem_info imsd_no_op_req_ei[] = {
	{
		.data_type = QMI_OPT_FLAG,
		.elem_len = 1,
		.elem_size = sizeof(bool),
		.tlv_type = 1,
		.offset = offsetof(struct imsd_no_op_req, data_valid),
	},
	{
		.data_type = QMI_UNSIGNED_1_BYTE,
		.elem_len = 255,
		.elem_size = sizeof(uint8_t),
		.array_type = STATIC_ARRAY,
		.tlv_type = 1,
		.offset = offsetof(struct imsd_no_op_req, data),
	},
	{}
};

struct qmi_elem_info imsd_no_op_resp_ei[] = {
	{
		.data_type = QMI_STRUCT,
		.elem_len = 1,
		.elem_size = sizeof(struct imsd_op_res),
		.tlv_type = 2,
		.offset = offsetof(struct imsd_no_op_resp, res),
		.ei_array = imsd_op_res_ei,
	},
	{}
};

